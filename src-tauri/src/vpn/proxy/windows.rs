//! Windows：从当前用户的 WinINET 设置读系统代理。
//!
//! 浏览器（Edge / Chrome / 大部分基于 WinINET 的程序）都读这里，和「设置 →
//! 网络和 Internet → 代理」是同一份数据。只读，不改用户设置。
//!
//! 说明：PAC（`AutoConfigURL`）不在这里处理 —— 它有脚本、无法静态判断会不会
//! 把域名交给代理，贸然报警只会制造噪音。

use super::common::ProxyState;
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

/// WinINET 的代理设置键。
const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

pub fn detect() -> ProxyState {
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(INTERNET_SETTINGS) else {
        return ProxyState::default();
    };
    let enabled = key
        .get_value::<u32, _>("ProxyEnable")
        .map(|value| value != 0)
        .unwrap_or(false);
    if !enabled {
        return ProxyState::default();
    }
    let server = key.get_value::<String, _>("ProxyServer").unwrap_or_default();
    let overrides = key
        .get_value::<String, _>("ProxyOverride")
        .unwrap_or_default();
    let servers = split_servers(&server);
    if servers.is_empty() {
        // ProxyEnable=1 但没填服务器：当作没开，别制造假报警
        return ProxyState::default();
    }
    ProxyState {
        enabled,
        servers,
        bypass: overrides
            .split(';')
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect(),
    }
}

/// `http=127.0.0.1:7897;https=127.0.0.1:7897` → `["127.0.0.1:7897"]`（去重）。
fn split_servers(raw: &str) -> Vec<String> {
    let mut servers: Vec<String> = Vec::new();
    for item in raw.split(';') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let addr = item
            .split_once('=')
            .map(|(_, addr)| addr.trim())
            .unwrap_or(item);
        if addr.is_empty() {
            continue;
        }
        if !servers.iter().any(|seen: &String| seen == addr) {
            servers.push(addr.to_string());
        }
    }
    servers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_protocol_prefixed_servers() {
        assert_eq!(
            split_servers("http=127.0.0.1:7897;https=127.0.0.1:7897;socks=127.0.0.1:7898"),
            vec![
                "127.0.0.1:7897".to_string(),
                "127.0.0.1:7898".to_string()
            ]
        );
        assert_eq!(split_servers("  "), Vec::<String>::new());
    }
}
