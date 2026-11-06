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

/// NRPT 的「子域后缀」命名空间：`risencn.com` → `.risencn.com`。
///
/// NRPT 里**不带前导点**的命名空间是「精确匹配」：规则 `risencn.com` 只命中
/// `risencn.com` 本身，`oa.risencn.com` 不会被它命中 —— 这也是现场「Windows 必须逐条
/// 写完整域名」的根因。带前导点才是「后缀匹配」，覆盖整族子域；微软自己的
/// VPN 配置示例也是用 `<DomainName>.corp.contoso.com</DomainName>` 表示后缀规则
/// （另一个不带点的 `<DomainName>hrsite.corporate.contoso.com</DomainName>` 是精确 FQDN）。
///
/// 和 macOS 的 `/etc/resolver/<域名>` 对齐：那边写一个域名就覆盖所有子域。
pub fn suffix_namespace(domain: &str) -> String {
    format!(".{}", domain)
}

/// 生成「先清掉自己的旧规则，再按域名逐条写 NRPT」的脚本（纯函数，便于单测）。
///
/// 每个域名写两条规则：
/// 1. `.域名` —— 后缀匹配，任何子域都命中（`DOMAIN-SUFFIX` 能覆盖子域的关键）；
/// 2. `域名` —— 精确匹配，保留旧行为，裸域名本身也要能解析。
///
/// 域名已经过 [`crate::vpn::split::normalize_domain`] 校验，只含字母数字、`-`、`_`、`.`，
/// 不会破坏单引号字符串；DNS 服务器是 `Ipv4Addr`，同样安全。
pub fn build_apply_script(domains: &[String], servers: &[Ipv4Addr]) -> String {
    let mut script = String::from(
        "$old = @(Get-DnsClientNrptRule -ErrorAction SilentlyContinue | \
         Where-Object { $_.DisplayName -like 'ws-workbench:*' }); \
         foreach ($r in $old) { Remove-DnsClientNrptRule -Name $r.Name -Force -ErrorAction SilentlyContinue }; \
         $added = 0; ",
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
        let suffix = suffix_namespace(domain);
        // 后缀规则是新增能力，但个别系统/安全软件可能拒绝带点的命名空间：
        // 它失败不能把整条 NRPT 布线（精确规则）一起拖垮，所以单独 try/catch。
        script.push_str(&format!(
            "try {{ Add-DnsClientNrptRule -Namespace '{suffix}' -NameServers {servers} \
             -DisplayName '{prefix}{suffix}' -Comment '{comment}' -ErrorAction Stop | Out-Null; \
             $added++ }} catch {{ }}; ",
            suffix = suffix,
            prefix = RULE_PREFIX,
            servers = server_list,
            comment = RULE_COMMENT
        ));
        // 精确规则保留旧行为；同样吞掉错误 —— 规则写不进去时由「成功条数为 0」
        // 和探针一起暴露，不能让一条规则把整条 NRPT 布线（含子域后缀）拖垮。
        script.push_str(&format!(
            "try {{ Add-DnsClientNrptRule -Namespace '{domain}' -NameServers {servers} \
             -DisplayName '{prefix}{domain}' -Comment '{comment}' -ErrorAction Stop | Out-Null; \
             $added++ }} catch {{ }}; ",
            prefix = RULE_PREFIX,
            domain = domain,
            servers = server_list,
            comment = RULE_COMMENT
        ));
    }
    script.push_str("$added");
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
/// 返回**写成功的规则条数**（每个域名两条：精确 + 子域后缀）。调用方用 0 判断
/// 「一条都没写进去」；后缀规则被系统拒绝时，这个数会比 `domains.len()*2` 小。
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
    let text = winps::run(&build_apply_script(domains, servers))?;
    Ok(parse_added_count(&text))
}

/// 从脚本输出里取最后一行数字（`$added`）。PowerShell 的警告可能混在 stdout 里，
/// 所以只认「整行就是数字」的最后一行，而不是直接 `trim().parse()`。
fn parse_added_count(text: &str) -> usize {
    text.lines()
        .rev()
        .find_map(|line| line.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

/// 列出当前生效的、属于我们的域名（`ws-workbench:` 前缀被打掉）。
///
/// 新版本每个域名有两条规则（`域名` 和 `.域名`），这里统一归一化成基础域名并去重，
/// 这样 `guard` 的「规则还在不在」判断不需要关心后缀规则是否存在。
pub fn installed_domains() -> Result<Vec<String>, String> {
    let text = winps::run(&build_list_script())?;
    let value = winps::parse_json(&text)?;
    let mut out = Vec::new();
    for item in winps::json_array(value) {
        if let Some(name) = item.as_str() {
            if let Some(domain) = display_name_to_domain(name) {
                if !out.contains(&domain) {
                    out.push(domain);
                }
            }
        }
    }
    Ok(out)
}

/// `ws-workbench:.risencn.com` / `ws-workbench:risencn.com` → `risencn.com`。
pub fn display_name_to_domain(display_name: &str) -> Option<String> {
    let raw = display_name.strip_prefix(RULE_PREFIX)?;
    let domain = raw.trim_start_matches('.').to_ascii_lowercase();
    if domain.is_empty() {
        return None;
    }
    Some(domain)
}

/// 列出当前生效的、属于我们的**原始命名空间**（带前导点会原样保留）。
///
/// 只用于现场诊断：用它能一眼确认系统到底存了 `.risencn.com` 还是把点吞掉了。
pub fn installed_namespaces() -> Result<Vec<String>, String> {
    let text = winps::run(&build_namespace_script())?;
    let value = winps::parse_json(&text)?;
    Ok(winps::json_array(value)
        .into_iter()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect())
}

/// 生成「列出我们自己的规则的 Namespace」的脚本，输出 JSON 数组。
fn build_namespace_script() -> String {
    format!(
        "$ns = @(Get-DnsClientNrptRule -ErrorAction SilentlyContinue | \
         Where-Object {{ $_.DisplayName -like '{}*' }} | ForEach-Object {{ $_.Namespace }}); \
         ConvertTo-Json -InputObject $ns -Compress",
        RULE_PREFIX
    )
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
            Ipv4Addr::new(172, 18, 160, 12),
            Ipv4Addr::new(172, 18, 160, 13),
        ]
    }

    #[test]
    fn apply_script_marks_its_own_rules_and_never_touches_others() {
        let script = build_apply_script(
            &["risencn.com".to_string(), "oa.risencn.com".to_string()],
            &ips(),
        );
        // 只按自己的 DisplayName 前缀清理
        assert!(script.contains("$_.DisplayName -like 'ws-workbench:*'"));
        assert!(script.contains("-DisplayName 'ws-workbench:risencn.com'"));
        assert!(script.contains("-DisplayName 'ws-workbench:oa.risencn.com'"));
        // 命名空间 + DNS 服务器都要写对
        assert!(script.contains("-Namespace 'risencn.com'"));
        assert!(script.contains("-NameServers '172.18.160.12','172.18.160.13'"));
        // 每个域名两条规则：精确 + 子域后缀
        assert_eq!(script.matches("Add-DnsClientNrptRule").count(), 4);
        // 绝不写注册表 / 组策略
        assert!(!script.contains("GpoName"));
        assert!(!script.contains("Set-DnsClientNrptRule"));
    }

    #[test]
    fn apply_script_covers_subdomains_with_a_dotted_namespace() {
        let script = build_apply_script(&["risencn.com".to_string()], &ips());
        // 不带点的规则只精确匹配 risencn.com；子域必须靠 .risencn.com
        assert!(script.contains("-Namespace '.risencn.com'"));
        assert!(script.contains("-DisplayName 'ws-workbench:.risencn.com'"));
        // 任何一条规则被系统拒绝都不能拖垮另一条：两条都包在 try/catch 里，
        // 用 $added 统计成功条数（0 条由调用方判定失败）。
        let dotted = script.find("try { Add-DnsClientNrptRule -Namespace '.risencn.com'");
        let exact = script.find("try { Add-DnsClientNrptRule -Namespace 'risencn.com'");
        assert!(dotted.is_some(), "后缀规则要能单独失败");
        assert!(exact.is_some(), "精确规则也要能单独失败");
        assert!(dotted < exact, "先写后缀规则，子域优先");
        assert!(script.contains("$added++"));
    }

    #[test]
    fn apply_script_counts_successful_rules() {
        let script = build_apply_script(&["risencn.com".to_string()], &ips());
        // $added 初值 0、每条成功规则 +1、脚本最后把数字输出给调用方
        assert!(script.contains("$added = 0;"));
        assert_eq!(script.matches("$added++").count(), 2);
        assert!(script.ends_with("$added"));
    }

    #[test]
    fn suffix_namespace_is_the_dotted_form() {
        assert_eq!(suffix_namespace("risencn.com"), ".risencn.com");
        assert_eq!(suffix_namespace("OA.RisenCN.com"), ".OA.RisenCN.com");
    }

    #[test]
    fn display_names_normalize_both_rule_kinds() {
        assert_eq!(
            display_name_to_domain("ws-workbench:risencn.com"),
            Some("risencn.com".to_string())
        );
        assert_eq!(
            display_name_to_domain("ws-workbench:.risencn.com"),
            Some("risencn.com".to_string())
        );
        assert_eq!(display_name_to_domain("ws-workbench:."), None);
        assert_eq!(display_name_to_domain("other:risencn.com"), None);
    }

    #[test]
    fn added_count_ignores_warning_lines() {
        assert_eq!(parse_added_count("4"), 4);
        assert_eq!(parse_added_count("WARNING: 某条规则被跳过\n3\n"), 3);
        assert_eq!(parse_added_count(""), 0);
        assert_eq!(parse_added_count("no digits here"), 0);
    }

    #[test]
    fn apply_script_can_point_at_the_local_forwarder() {
        let script =
            build_apply_script(&["risencn.com".to_string()], &[Ipv4Addr::new(127, 0, 0, 1)]);
        assert!(script.contains("-Namespace 'risencn.com'"));
        assert!(script.contains("-Namespace '.risencn.com'"));
        assert!(script.contains("-NameServers '127.0.0.1'"));
    }

    #[test]
    fn apply_script_without_dns_is_a_noop_cleanup() {
        let script = build_apply_script(&["risencn.com".to_string()], &[]);
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
        let domains = vec!["risencn.com".to_string(), "oa.risencn.com".to_string()];
        let scripts = vec![
            ("apply", build_apply_script(&domains, &ips())),
            ("list", build_list_script()),
            ("remove", build_remove_script()),
            (
                "onlink",
                super::super::route::windows_ensure_onlink_script(
                    &["192.168.136.0/22".to_string(), "10.8.0.12/32".to_string()],
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
