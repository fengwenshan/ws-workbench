//! 提权删除脚本里的纯字符串逻辑（所有平台都编译，单测在 macOS 上也能跑）。
//!
//! 平台文件只负责调用，不重复实现转义；这样 Windows 的 PowerShell 引号规则
//! 在非 Windows 机器上也有测试覆盖。

/// PowerShell 单引号字符串转义：`'` → `''`。
///
/// 注意与 POSIX 的 `'\''` 完全不同：在 PowerShell 里往单引号字符串中塞
/// `'\''` 会把字符串提前闭合，路径里的单引号于是能变成命令分隔符。
/// Windows 提权脚本里所有路径、注册表键、临时文件路径都必须过这个函数。
/// 非 Windows 平台只有单测会用到，lib 构建里允许死代码。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(super) fn powershell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}
