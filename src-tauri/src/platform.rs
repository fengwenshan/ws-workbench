//! 操作系统判定：全项目唯一的平台真相来源
//!
//! 之前每个模块各写各的 `#[cfg(target_os = "windows")]` / `cfg!(...)`，
//! 还出现过「不是 windows 就当成 macos」这种在 Linux 上会直接出错的假设
//! （Linux 上会去找 homebrew 的路径）。
//!
//! 从现在起，平台相关的判断一律走本模块：
//! - 编译期分支仍然用 `#[cfg(...)]`，但语义判断统一用 [`Platform::is_windows`] 这类方法
//! - 需要在界面上展示平台信息时，用 [`PlatformInfo`]，不要在前端猜 userAgent
//!
//! 前端对应物是 `src/platform.ts`，两边通过 `get_platform_info` 命令对齐。

use serde::{Deserialize, Serialize};

/// 支持的操作系统
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Windows,
    Macos,
    Linux,
}

/// 当前操作系统（前端直接拿到这个枚举的字符串形式）
#[derive(Debug, Clone, Copy)]
pub struct Platform;

impl Platform {
    /// 编译期就能确定的当前平台
    pub const CURRENT: Os = Os::current();

    pub const fn is_windows(&self) -> bool {
        matches!(Self::CURRENT, Os::Windows)
    }

    pub const fn is_macos(&self) -> bool {
        matches!(Self::CURRENT, Os::Macos)
    }

    pub const fn is_linux(&self) -> bool {
        matches!(Self::CURRENT, Os::Linux)
    }

    /// Windows 专有的查找逻辑（注册表、App Paths、PATHEXT 等）只在 Windows 生效
    pub const fn has_windows_app_search(&self) -> bool {
        Self::is_windows(&Self)
    }

    /// 可执行文件后缀
    pub const fn exe_suffix(&self) -> &'static str {
        match Self::CURRENT {
            Os::Windows => ".exe",
            _ => "",
        }
    }

    /// 建立 VPN 虚拟网卡、修改路由表是否需要管理员权限
    pub const fn vpn_needs_admin(&self) -> bool {
        // Windows 要管理员才能建 Wintun/TAP 网卡并改路由；
        // macOS 建 utun 需要 root；Linux 通常需要 root 或 CAP_NET_ADMIN
        true
    }
}

impl Os {
    pub const fn current() -> Os {
        #[cfg(target_os = "windows")]
        {
            Os::Windows
        }
        #[cfg(target_os = "macos")]
        {
            Os::Macos
        }
        // 其余一律归为 Linux。不要在这里写「非 windows 即 macos」，
        // 那会让 Linux 走到 brew 的查找路径上。
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            Os::Linux
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::Macos => "macos",
            Os::Linux => "linux",
        }
    }

    pub const fn label(&self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::Macos => "macOS",
            Os::Linux => "Linux",
        }
    }
}

/// 提供给前端的平台信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInfo {
    /// windows / macos / linux
    pub os: Os,
    /// 展示名：Windows / macOS / Linux
    pub label: String,
    /// 目标架构，例如 x86_64
    pub arch: String,
    /// 编译目标三元组，例如 x86_64-pc-windows-msvc
    pub target: String,
    pub is_windows: bool,
    pub is_macos: bool,
    pub is_linux: bool,
    /// 可执行文件后缀
    pub exe_suffix: String,
    /// VPN 是否必须提权
    pub vpn_needs_admin: bool,
    /// 是否支持「手动指定应用路径」这类 Windows 专有设置
    pub supports_app_path_settings: bool,
}

impl PlatformInfo {
    pub fn current() -> Self {
        // 全部经由 Platform 取值，保证与全项目其它地方判断一致
        let os = Platform::CURRENT;
        PlatformInfo {
            os,
            label: os.label().to_string(),
            arch: std::env::consts::ARCH.to_string(),
            target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
            is_windows: Platform::is_windows(&Platform),
            is_macos: Platform::is_macos(&Platform),
            is_linux: Platform::is_linux(&Platform),
            exe_suffix: Platform::exe_suffix(&Platform).to_string(),
            vpn_needs_admin: Platform::vpn_needs_admin(&Platform),
            supports_app_path_settings: Platform::has_windows_app_search(&Platform),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_matches_compile_target() {
        let os = Os::current();
        if cfg!(target_os = "windows") {
            assert_eq!(os, Os::Windows);
        } else if cfg!(target_os = "macos") {
            assert_eq!(os, Os::Macos);
        } else {
            // 关键回归点：Linux 不能被当成 macOS
            assert_eq!(os, Os::Linux);
        }
    }

    #[test]
    fn only_one_platform_flag_is_true() {
        let flags = [
            Platform::is_windows(&Platform),
            Platform::is_macos(&Platform),
            Platform::is_linux(&Platform),
        ];
        assert_eq!(flags.iter().filter(|flag| **flag).count(), 1);
    }

    #[test]
    fn exe_suffix_matches_platform() {
        let suffix = Platform::exe_suffix(&Platform);
        if cfg!(target_os = "windows") {
            assert_eq!(suffix, ".exe");
        } else {
            assert_eq!(suffix, "");
        }
    }

    #[test]
    fn app_path_settings_are_windows_only() {
        assert_eq!(
            Platform::has_windows_app_search(&Platform),
            cfg!(target_os = "windows")
        );
    }

    #[test]
    fn as_str_matches_frontend_contract() {
        // 这些字符串会直接出现在发给前端的 PlatformInfo 里，
        // 前端 src/platform.ts 按同样的字面量判断，改动需要两边同步
        assert_eq!(Os::Windows.as_str(), "windows");
        assert_eq!(Os::Macos.as_str(), "macos");
        assert_eq!(Os::Linux.as_str(), "linux");
    }

    #[test]
    fn platform_info_is_consistent() {
        let info = PlatformInfo::current();
        assert_eq!(info.os, Os::current());
        assert_eq!(info.is_windows, matches!(info.os, Os::Windows));
        assert_eq!(info.is_macos, matches!(info.os, Os::Macos));
        assert_eq!(info.is_linux, matches!(info.os, Os::Linux));
        assert_eq!(info.label, Os::current().label());
        assert!(!info.arch.is_empty());
    }

    #[test]
    fn labels_are_distinct() {
        assert_ne!(Os::Windows.label(), Os::Macos.label());
        assert_ne!(Os::Macos.label(), Os::Linux.label());
        assert_ne!(Os::Windows.label(), Os::Linux.label());
    }
}
