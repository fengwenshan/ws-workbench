//! Windows 的「按域名分流解析」与接口 DNS
//!
//! 这是 macOS 侧两个系统能力的 Windows 对应物：
//!
//! | macOS | Windows |
//! | --- | --- |
//! | `/etc/resolver/<域名>` 指向 DNS 服务器 | **NRPT**（Name Resolution Policy Table）按域名指定 DNS |
//! | `networksetup -setdnsservers` / `scutil` | `Set-DnsClientServerAddress` |
//! | `dscacheutil -flushcache` + HUP mDNSResponder | `Clear-DnsClientCache` |
//!
//! NRPT 是 Windows 官方的「按命名空间解析」机制：一条规则 = 一个域名 + 一组 DNS
//! 服务器，只影响命中该域名的查询，不改系统 DNS、不动默认路由。这正好是规则模式
//! 需要的东西 —— 命中「走 VPN」的域名交给 VPN DNS，其余域名仍由本机原来的 DNS 解析。
//!
//! 两条硬约束：
//! 1. NRPT 由 DNS Client 服务（`Dnscache`）执行，服务没跑规则就不生效，所以写规则
//!    之前先确认它在跑；起不来就退回「只给隧道网卡配 DNS」并如实记日志。
//! 2. 规则用 `DisplayName` 前缀标记归属，删除时只删自己的。绝不能按命名空间
//!    「猜」哪些规则是用户配的 —— 公司下发的 DirectAccess 规则也在同一个表里。

use std::net::Ipv4Addr;

use crate::winps;

/// 我们写的 NRPT 规则的 `DisplayName` 前缀。
///
/// 前缀同时是归属标记：清理只认它，不会碰用户 / 域策略下发的规则。
pub const RULE_PREFIX: &str = "ws-workbench:";
/// 规则的 `Comment`，排查时一眼能看出是谁写的
pub const RULE_COMMENT: &str = "ws-workbench";

/// DNS Client 服务在不在跑（NRPT 的执行者）。
pub fn dns_client_running() -> bool {
    match winps::run("(Get-Service -Name Dnscache -ErrorAction Stop).Status") {
        Ok(text) => text.trim().eq_ignore_ascii_case("Running"),
        Err(_) => false,
    }
}

/// 尽量把 DNS Client 服务拉起来。返回是否在跑。
///
/// 服务被域策略禁用时 `Start-Service` 也会失败，这里只记录结果，
/// 由调用方决定降级方式，不把「DNS 服务不可用」变成连接失败。
pub fn ensure_dns_client() -> bool {
    if dns_client_running() {
        return true;
    }
    let _ = winps::run("Start-Service -Name Dnscache -ErrorAction SilentlyContinue");
    dns_client_running()
}

/// 清空本机 DNS 缓存（改了 NRPT 或接口 DNS 之后必须做，否则旧答案还在）。
pub fn flush_cache() {
    let _ = winps::run("Clear-DnsClientCache -ErrorAction SilentlyContinue");
}

/// 生成「先清掉自己的旧规则，再按域名逐条写 NRPT」的脚本（纯函数，便于单测）。
///
/// 域名已经过 [`crate::vpn::split::normalize_domain`] 校验，只含字母数字、`-`、`_`、`.`，
/// 不会破坏单引号字符串；DNS 服务器是 `Ipv4Addr`，同样安全。
pub fn build_apply_script(domains: &[String], servers: &[Ipv4Addr]) -> String {
    let mut script = String::from(
        "$old = @(Get-DnsClientNrptRule -ErrorAction SilentlyContinue | \
         Where-Object { $_.DisplayName -like 'ws-workbench:*' }); \
         foreach ($r in $old) { Remove-DnsClientNrptRule -Name $r.Name -Force -ErrorAction SilentlyContinue }; ",
    );
    if servers.is_empty() {
        return script;
    }
    let server_list = servers
        .iter()
        .map(|ip| format!("'{}'", ip))
        .collect::<Vec<_>>()
        .join(",");
    for domain in domains {
        // 先删同名规则再建：NRPT 允许同一个命名空间存在多条规则，
        // 重复添加会让「哪条生效」变成未定义行为。
        script.push_str(&format!(
            "Get-DnsClientNrptRule -ErrorAction SilentlyContinue | \
             Where-Object {{ $_.DisplayName -eq '{prefix}{domain}' }} | \
             ForEach-Object {{ Remove-DnsClientNrptRule -Name $_.Name -Force -ErrorAction SilentlyContinue }}; \
             Add-DnsClientNrptRule -Namespace '{domain}' -NameServers {servers} \
             -DisplayName '{prefix}{domain}' -Comment '{comment}' -ErrorAction Stop | Out-Null; ",
            prefix = RULE_PREFIX,
            domain = domain,
            servers = server_list,
            comment = RULE_COMMENT
        ));
    }
    script
}

/// 生成「列出我们自己的规则」的脚本，输出 JSON 数组。
pub fn build_list_script() -> String {
    format!(
        "$names = @(Get-DnsClientNrptRule -ErrorAction SilentlyContinue | \
         Where-Object {{ $_.DisplayName -like '{}*' }} | ForEach-Object {{ $_.DisplayName }}); \
         ConvertTo-Json -InputObject $names -Compress",
        RULE_PREFIX
    )
}

/// 生成「删掉我们自己的全部规则」的脚本。
pub fn build_remove_script() -> String {
    format!(
        "$old = @(Get-DnsClientNrptRule -ErrorAction SilentlyContinue | \
         Where-Object {{ $_.DisplayName -like '{}*' }}); \
         $n = 0; foreach ($r in $old) {{ \
           Remove-DnsClientNrptRule -Name $r.Name -Force -ErrorAction SilentlyContinue; $n++ }}; $n",
        RULE_PREFIX
    )
}

/// 按域名写 NRPT 规则：只影响这些域名的解析，其余域名照旧。
///
/// 返回写成功的规则数。任何一步失败都返回错误，由调用方决定降级；
/// 不会留下「一半域名生效、一半没有」的中间态太久（脚本本身就是先删后建）。
pub fn apply_namespace_rules(domains: &[String], servers: &[Ipv4Addr]) -> Result<usize, String> {
    if domains.is_empty() {
        return Ok(0);
    }
    if servers.is_empty() {
        return Err("服务端没有下发 DNS，无法为内网域名配置解析".to_string());
    }
    if !ensure_dns_client() {
        return Err("DNS Client 服务（Dnscache）没有运行，NRPT 规则不会生效".to_string());
    }
    winps::run(&build_apply_script(domains, servers))?;
    Ok(domains.len())
}

/// 列出当前生效的、属于我们的域名（`ws-workbench:` 前缀被打掉）。
pub fn installed_domains() -> Result<Vec<String>, String> {
    let text = winps::run(&build_list_script())?;
    let value = winps::parse_json(&text)?;
    let mut out = Vec::new();
    for item in winps::json_array(value) {
        if let Some(name) = item.as_str() {
            if let Some(domain) = name.strip_prefix(RULE_PREFIX) {
                out.push(domain.to_string());
            }
        }
    }
    Ok(out)
}

/// 删掉我们写过的全部 NRPT 规则，返回删掉的条数。
///
/// 也用于崩溃恢复：worker 被强杀时规则会留着，下次连接前必须先收干净，
/// 否则内网域名会一直指向一个已经断掉的隧道（解析超时）。
pub fn remove_all_rules() -> Result<usize, String> {
    let text = winps::run(&build_remove_script())?;
    Ok(text.trim().parse::<usize>().unwrap_or(0))
}

/// 把隧道网卡的 DNS 设成服务端下发的地址（全隧道模式用）。
///
/// 只动隧道网卡，不碰物理网卡：物理网卡的 DNS 查询在 /1 路由下本来就走隧道，
/// 改它反而会在 VPN DNS 暂时不可达时把整机解析拖死（macOS 那边是为了绕开
/// mDNSResponder 把查询钉在物理网卡上的问题，Windows 没有这个问题）。
pub fn set_interface_dns(if_index: u32, servers: &[Ipv4Addr]) -> Result<(), String> {
    if servers.is_empty() {
        return Err("服务端没有下发 DNS".to_string());
    }
    if if_index == 0 {
        return Err("隧道接口索引为 0，无法设置 DNS".to_string());
    }
    let list = servers
        .iter()
        .map(|ip| format!("'{}'", ip))
        .collect::<Vec<_>>()
        .join(",");
    winps::run(&format!(
        "Set-DnsClientServerAddress -InterfaceIndex {} -ServerAddresses {} -ErrorAction Stop",
        if_index, list
    ))
    .map(|_| ())
}

/// 把隧道网卡的 DNS 还原成「自动获取」。
pub fn reset_interface_dns(if_index: u32) -> Result<(), String> {
    if if_index == 0 {
        return Ok(());
    }
    winps::run(&format!(
        "Set-DnsClientServerAddress -InterfaceIndex {} -ResetServerAddresses -ErrorAction Stop",
        if_index
    ))
    .map(|_| ())
}

/// 读隧道网卡当前的 DNS（守护用：被别的软件改掉时补回来）。
pub fn interface_dns(if_index: u32) -> Result<Vec<String>, String> {
    if if_index == 0 {
        return Ok(Vec::new());
    }
    let text = winps::run(&format!(
        "$s = @(Get-DnsClientServerAddress -InterfaceIndex {} -AddressFamily IPv4 \
         -ErrorAction Stop | Select-Object -ExpandProperty ServerAddresses); \
         ConvertTo-Json -InputObject $s -Compress",
        if_index
    ))?;
    let value = winps::parse_json(&text)?;
    Ok(winps::json_array(value)
        .into_iter()
        .filter_map(|item| item.as_str().map(|s| s.to_string()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ips() -> Vec<Ipv4Addr> {
        vec![
            Ipv4Addr::new(192, 0, 2, 12),
            Ipv4Addr::new(192, 0, 2, 13),
        ]
    }

    #[test]
    fn apply_script_marks_its_own_rules_and_never_touches_others() {
        let script = build_apply_script(
            &["example.com".to_string(), "oa.example.com".to_string()],
            &ips(),
        );
        // 只按自己的 DisplayName 前缀清理
        assert!(script.contains("$_.DisplayName -like 'ws-workbench:*'"));
        assert!(script.contains("-DisplayName 'ws-workbench:example.com'"));
        assert!(script.contains("-DisplayName 'ws-workbench:oa.example.com'"));
        // 命名空间 + DNS 服务器都要写对
        assert!(script.contains("-Namespace 'example.com'"));
        assert!(script.contains("-NameServers '192.0.2.12','192.0.2.13'"));
        // 先删同名规则再建，避免同一命名空间多条规则
        assert_eq!(script.matches("Add-DnsClientNrptRule").count(), 2);
        // 绝不写注册表 / 组策略
        assert!(!script.contains("GpoName"));
        assert!(!script.contains("Set-DnsClientNrptRule"));
    }

    #[test]
    fn apply_script_can_point_at_the_local_forwarder() {
        let script =
            build_apply_script(&["example.com".to_string()], &[Ipv4Addr::new(127, 0, 0, 1)]);
        assert!(script.contains("-Namespace 'example.com'"));
        assert!(script.contains("-NameServers '127.0.0.1'"));
    }

    #[test]
    fn apply_script_without_dns_is_a_noop_cleanup() {
        let script = build_apply_script(&["example.com".to_string()], &[]);
        assert!(script.contains("Remove-DnsClientNrptRule"));
        assert!(
            !script.contains("Add-DnsClientNrptRule"),
            "没有 VPN DNS 时不能写规则，否则域名会指向不存在的服务器"
        );
    }

    #[test]
    fn remove_script_only_matches_our_prefix() {
        let script = build_remove_script();
        assert!(script.contains("$_.DisplayName -like 'ws-workbench:*'"));
        assert!(script.contains("Remove-DnsClientNrptRule -Name $r.Name -Force"));
        // 删规则时不能按命名空间猜：DirectAccess 的规则也在同一张表里
        assert!(!script.contains("-Namespace"));
    }

    #[test]
    fn list_script_returns_json_array() {
        let script = build_list_script();
        assert!(script.contains("ConvertTo-Json -InputObject $names"));
        assert!(script.contains("-like 'ws-workbench:*'"));
    }

    /// 生成的 PowerShell 必须真的能被解析。
    ///
    /// 字符串断言只保证「有某个片段」，括号/引号配错、`if` 少了花括号这类错误
    /// 它一个都发现不了；而真机上脚本报语法错误的后果是「静默不生效」，
    /// 只会表现成「分流没反应」，很难从日志定位。这里只解析、不执行，安全。
    #[cfg(target_os = "windows")]
    #[test]
    fn generated_scripts_parse_as_powershell() {
        let domains = vec!["example.com".to_string(), "oa.example.com".to_string()];
        let scripts = vec![
            ("apply", build_apply_script(&domains, &ips())),
            ("list", build_list_script()),
            ("remove", build_remove_script()),
            (
                "onlink",
                super::super::route::windows_ensure_onlink_script(
                    &["198.51.100.0/24".to_string(), "10.10.0.12/32".to_string()],
                    21,
                    1,
                )
                .unwrap(),
            ),
            (
                "remove-stale",
                super::super::route::windows_remove_vpn_prefix_script(&[
                    "0.0.0.0/1".to_string(),
                    "128.0.0.0/1".to_string(),
                ]),
            ),
        ];
        for (name, script) in scripts {
            // 走环境变量传脚本，避开命令行引号转义；脚本本身是纯 ASCII
            let out = std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "[void][ScriptBlock]::Create($env:WS_SCRIPT_UNDER_TEST); 'parsed'",
                ])
                .env("WS_SCRIPT_UNDER_TEST", &script)
                .output()
                .expect("powershell.exe 是 Windows 自带组件");
            assert!(
                out.status.success(),
                "{} 脚本语法错误：{}",
                name,
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                String::from_utf8_lossy(&out.stdout).contains("parsed"),
                "{} 脚本没有解析成功",
                name
            );
        }
    }
}
