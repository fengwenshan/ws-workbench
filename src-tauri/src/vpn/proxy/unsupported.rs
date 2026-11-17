//! 其它平台：没有系统代理要检测（域名分流本身也不支持）。

use super::common::ProxyState;

/// 永远返回「没开代理」。
pub fn detect() -> ProxyState {
    ProxyState::default()
}
