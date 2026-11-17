//! macOS：从 `scutil --proxy` 读系统代理。
//!
//! 这是「系统设置 → 网络 → 代理」的同一份数据，Chromium 系浏览器与
//! `NSURLSession` 都读它。只读，不改用户设置。
//!
//! `scutil --proxy` 的输出是稳定的 ASCII 键值，不是本地化文本，可以直接解析。
//! PAC（`ProxyAutoConfigEnable`）与 Windows 侧同样不处理：脚本里会不会把域名
//! 交给代理无法静态判断，报警只会制造噪音。

#[cfg(target_os = "macos")]
use std::process::Command;

use super::common::ProxyState;

/// `scutil` 在 macOS 上的固定位置（不依赖 PATH）。
#[cfg(target_os = "macos")]
const SCUTIL: &str = "/usr/sbin/scutil";

#[cfg(target_os = "macos")]
pub fn detect() -> ProxyState {
    let output = match Command::new(SCUTIL).arg("--proxy").output() {
        Ok(output) if output.status.success() => output,
        _ => return ProxyState::default(),
    };
    parse_scutil(&String::from_utf8_lossy(&output.stdout))
}

/// 解析 `scutil --proxy` 的输出（纯函数，便于在别的平台跑单测）。
#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_scutil(text: &str) -> ProxyState {
    let mut state = ProxyState::default();
    let mut in_exceptions = false;
    let mut proxy_hosts: Vec<(String, String)> = Vec::new();
    let mut proxy_ports: Vec<(String, String)> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if in_exceptions {
            if line == "}" {
                in_exceptions = false;
                continue;
            }
            if let Some((_, item)) = line.split_once(':') {
                let item = item.trim().trim_matches('"');
                if !item.is_empty() {
                    state.bypass.push(item.to_string());
                }
            }
            continue;
        }
        if line.starts_with("ExceptionsList") {
            in_exceptions = true;
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        if value.is_empty() {
            continue;
        }
        match key {
            "HTTPEnable" | "HTTPSEnable" | "SOCKSEnable" => {
                if value == "1" {
                    state.enabled = true;
                }
            }
            "HTTPProxy" | "HTTPSProxy" | "SOCKSProxy" => {
                proxy_hosts.push((key.to_string(), value.to_string()));
            }
            "HTTPPort" | "HTTPSPort" | "SOCKSPort" => {
                proxy_ports.push((key.to_string(), value.to_string()));
            }
            _ => {}
        }
    }

    for (host_key, host) in &proxy_hosts {
        let port_key = host_key.replace("Proxy", "Port");
        let port = proxy_ports
            .iter()
            .find(|(key, _)| *key == port_key)
            .map(|(_, port)| port.clone());
        let server = match port {
            Some(port) if !port.is_empty() => format!("{}:{}", host, port),
            _ => host.clone(),
        };
        if !state.servers.iter().any(|seen| *seen == server) {
            state.servers.push(server);
        }
    }

    if state.servers.is_empty() {
        // 只开了 SOCKS、或 Enable 位开了却没填服务器：都不值得报警
        state.enabled = false;
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "<dictionary> {
  ExceptionsList : <array> {
    0 : *.local
    1 : 169.254/16
    2 : 192.168.0.0/16
  }
  FTPPassive : 1
  HTTPEnable : 1
  HTTPPort : 7890
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 1
  HTTPSPort : 7890
  HTTPSProxy : 127.0.0.1
  ProxyAutoConfigEnable : 0
  SOCKSEnable : 0
}";

    #[test]
    fn parses_enabled_proxy_and_exceptions() {
        let state = parse_scutil(SAMPLE);
        assert!(state.enabled);
        // HTTP 与 HTTPS 是同一个地址，去重后只剩一条
        assert_eq!(state.servers, vec!["127.0.0.1:7890".to_string()]);
        assert_eq!(
            state.bypass,
            vec![
                "*.local".to_string(),
                "169.254/16".to_string(),
                "192.168.0.0/16".to_string()
            ]
        );
    }

    #[test]
    fn disabled_proxy_means_no_state() {
        let state = parse_scutil("HTTPEnable : 0\nHTTPProxy : 127.0.0.1\nHTTPPort : 7890\n");
        assert!(!state.enabled);
        assert!(state.servers.is_empty());
    }

    #[test]
    fn enable_bit_without_server_is_not_reported() {
        let state = parse_scutil("HTTPEnable : 1\n");
        assert!(!state.enabled);
    }
}
