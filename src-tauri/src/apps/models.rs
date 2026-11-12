//! 需要定位的外部应用，以及「怎么找它」的全部元数据
//!
//! 之前这份知识散落在两处：`commands.rs` 里的 win_launcher 负责 VS Code / IDEA / Trae，
//! `vpn/platform` 负责 openconnect，两边各写一份 PATH 查找和目录扫描。
//! 现在统一到这张表里，新增一个应用只需要在这里加一项。

use serde::{Deserialize, Serialize};

/// 应用标识
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppId {
    Vscode,
    Idea,
    Pycharm,
    Webstorm,
    Goland,
    Rustrover,
    Cursor,
    Trae,
    /// DeepSeek Harness 桌面版：点「打开方式 → DeepSeek」时把目录作为工作区交给它
    DeepSeek,
    /// VPN 唯一的连接程序：命令行版 openconnect。
    /// Windows 上优先用随程序分发的内置副本，macOS 上走 Homebrew 装的那份。
    Openconnect,
}

impl AppId {
    pub const ALL: &'static [AppId] = &[
        AppId::Vscode,
        AppId::Idea,
        AppId::Pycharm,
        AppId::Webstorm,
        AppId::Goland,
        AppId::Rustrover,
        AppId::Cursor,
        AppId::Trae,
        AppId::DeepSeek,
        AppId::Openconnect,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            AppId::Vscode => "vscode",
            AppId::Idea => "idea",
            AppId::Pycharm => "pycharm",
            AppId::Webstorm => "webstorm",
            AppId::Goland => "goland",
            AppId::Rustrover => "rustrover",
            AppId::Cursor => "cursor",
            AppId::Trae => "trae",
            AppId::DeepSeek => "deepseek",
            AppId::Openconnect => "openconnect",
        }
    }

    pub fn from_str(text: &str) -> Option<AppId> {
        AppId::ALL
            .iter()
            .copied()
            .find(|id| id.as_str() == text.trim().to_lowercase())
    }

    pub const fn display_name(&self) -> &'static str {
        match self {
            AppId::Vscode => "Visual Studio Code",
            AppId::Idea => "IntelliJ IDEA",
            AppId::Pycharm => "PyCharm",
            AppId::Webstorm => "WebStorm",
            AppId::Goland => "GoLand",
            AppId::Rustrover => "RustRover",
            AppId::Cursor => "Cursor",
            AppId::Trae => "Trae",
            AppId::DeepSeek => "DeepSeek",
            AppId::Openconnect => "OpenConnect（命令行）",
        }
    }

    pub const fn description(&self) -> &'static str {
        match self {
            AppId::Vscode => "在 VS Code 里打开项目目录",
            AppId::Idea => "在 IntelliJ IDEA 里打开项目目录",
            AppId::Pycharm => "在 PyCharm 里打开项目目录",
            AppId::Webstorm => "在 WebStorm 里打开项目目录",
            AppId::Goland => "在 GoLand 里打开项目目录",
            AppId::Rustrover => "在 RustRover 里打开项目目录",
            AppId::Cursor => "在 Cursor 里打开项目目录",
            AppId::Trae => "在 Trae 里打开项目目录",
            AppId::DeepSeek => "在 DeepSeek Harness 里打开项目工作区",
            AppId::Openconnect => {
                "VPN 唯一的连接程序：支持免手输密码自动登录，\
                 也只有它能按网段精确分流。"
            }
        }
    }

    /// 可执行文件名，按优先级排列。
    /// Windows 上 .cmd/.bat 是命令行包装脚本，需要交给 cmd.exe 解释。
    pub const fn executable_names(&self) -> &'static [&'static str] {
        match self {
            AppId::Vscode => &["Code.exe", "code.cmd", "code.bat", "code"],
            AppId::Idea => &["idea64.exe", "idea.exe", "idea.cmd", "idea.bat"],
            AppId::Pycharm => &["pycharm64.exe", "pycharm.exe", "pycharm.cmd", "pycharm.bat"],
            AppId::Webstorm => &["webstorm64.exe", "webstorm.exe", "webstorm.cmd", "webstorm.bat"],
            AppId::Goland => &["goland64.exe", "goland.exe", "goland.cmd", "goland.bat"],
            AppId::Rustrover => &[
                "rustrover64.exe",
                "rustrover.exe",
                "rustrover.cmd",
                "rustrover.bat",
            ],
            AppId::Cursor => &["Cursor.exe", "cursor.cmd", "cursor.bat", "cursor"],
            AppId::Trae => &[
                "Trae CN.exe",
                "Trae.exe",
                "trae-solo-cn.exe",
                "trae-solo-cn.cmd",
                "trae.exe",
                "trae-cn.cmd",
                "trae.cmd",
                "trae.bat",
            ],
            AppId::DeepSeek => &["DeepSeek Harness.exe", "DeepSeek.exe"],
            AppId::Openconnect => &["openconnect.exe", "openconnect"],
        }
    }

    /// Windows 注册表里用来匹配 DisplayName 的关键词（不区分大小写）
    pub const fn registry_keywords(&self) -> &'static [&'static str] {
        match self {
            AppId::Vscode => &["Visual Studio Code", "VS Code", "VSCode"],
            AppId::Idea => &["IntelliJ IDEA", "JetBrains Toolbox"],
            AppId::Pycharm => &["PyCharm"],
            AppId::Webstorm => &["WebStorm"],
            AppId::Goland => &["GoLand"],
            AppId::Rustrover => &["RustRover"],
            AppId::Cursor => &["Cursor"],
            AppId::Trae => &["Trae"],
            AppId::DeepSeek => &["DeepSeek Harness"],
            AppId::Openconnect => &["OpenConnect"],
        }
    }

    /// 相对于某个安装根目录（Program Files / LOCALAPPDATA 等）的固定子目录，
    /// 可执行文件就放在里面，或在它的 bin 子目录里
    pub const fn windows_install_dirs(&self) -> &'static [&'static str] {
        match self {
            AppId::Vscode => &[
                "Microsoft VS Code",
                "VSCode",
                "Programs\\Microsoft VS Code",
                "Programs\\VSCode",
                "Programs\\Microsoft VS Code Insiders",
            ],
            AppId::Idea
            | AppId::Pycharm
            | AppId::Webstorm
            | AppId::Goland
            | AppId::Rustrover => &["JetBrains\\Toolbox\\scripts"],
            AppId::Cursor => &["Programs\\cursor", "Cursor"],
            AppId::Trae => &[
                "TRAE SOLO CN",
                "Trae CN",
                "Trae",
                "Programs\\TRAE SOLO CN",
                "Programs\\Trae CN",
                "Programs\\Trae",
            ],
            AppId::DeepSeek => &[
                "DeepSeek Harness",
                "Programs\\DeepSeek Harness",
                "DeepSeek",
                "Programs\\DeepSeek",
            ],
            AppId::Openconnect => &["OpenConnect", "openconnect"],
        }
    }

    /// 名字里含这些关键词的目录需要再往下找（JetBrains 的目录带版本号）
    pub const fn windows_versioned_dirs(&self) -> &'static [&'static str] {
        match self {
            AppId::Idea => &["IntelliJ IDEA", "IDEA"],
            AppId::Pycharm => &["PyCharm"],
            AppId::Webstorm => &["WebStorm"],
            AppId::Goland => &["GoLand"],
            AppId::Rustrover => &["RustRover"],
            // 手工解压出来的目录通常带版本号，例如 openconnect-9.21_MINGW64
            AppId::Openconnect => &["openconnect"],
            _ => &[],
        }
    }

    /// 版本化目录里的最大搜索深度
    pub const fn windows_search_depth(&self) -> usize {
        match self {
            AppId::Idea
            | AppId::Pycharm
            | AppId::Webstorm
            | AppId::Goland
            | AppId::Rustrover => 4,
            _ => 2,
        }
    }

    /// macOS 下的固定位置。Linux 走 PATH + 常见前缀，不单独列举。
    pub const fn macos_paths(&self) -> &'static [&'static str] {
        match self {
            AppId::Vscode => &[
                "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
                "/opt/homebrew/bin/code",
                "/usr/local/bin/code",
            ],
            AppId::Idea => &[
                "/Applications/IntelliJ IDEA.app/Contents/MacOS/idea",
                "/Applications/IntelliJ IDEA CE.app/Contents/MacOS/idea",
            ],
            AppId::Pycharm => &[
                "/Applications/PyCharm.app/Contents/MacOS/pycharm",
                "/Applications/PyCharm CE.app/Contents/MacOS/pycharm",
            ],
            AppId::Webstorm => &["/Applications/WebStorm.app/Contents/MacOS/webstorm"],
            AppId::Goland => &["/Applications/GoLand.app/Contents/MacOS/goland"],
            AppId::Rustrover => &["/Applications/RustRover.app/Contents/MacOS/rustrover"],
            AppId::Cursor => &["/Applications/Cursor.app/Contents/MacOS/Cursor"],
            AppId::Trae => &["/Applications/Trae.app/Contents/MacOS/Trae"],
            AppId::DeepSeek => &[
                "/Applications/DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness",
            ],
            AppId::Openconnect => &[
                "/opt/homebrew/bin/openconnect",
                "/opt/homebrew/opt/openconnect/bin/openconnect",
                "/usr/local/bin/openconnect",
                "/opt/local/bin/openconnect",
            ],
        }
    }
}

/// 找到的一个候选
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppCandidate {
    pub path: String,
    /// override / managed / bundled / registry / system / path
    pub source: String,
    /// 该路径当前是否存在
    pub exists: bool,
}

/// 一个应用的定位结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLocation {
    pub id: String,
    pub name: String,
    pub description: String,
    /// 最终选中的可执行文件路径；空表示没找到
    pub path: String,
    pub available: bool,
    /// custom(调用方指定) / managed(托管副本) / registry / system / path / bundled / missing
    pub source: String,
    /// 补充说明：找不到时的排查提示，或来源说明
    pub note: String,
    /// 随程序分发的内置副本路径。
    ///
    /// 单独一个字段，**不参与「查找」**：它的位置在编译期就确定了，
    /// 没有任何可搜的余地。混在候选列表里只会让人以为还得找一遍。
    #[serde(default)]
    pub builtin_path: String,
    /// 其它找到的候选（不含内置副本）
    pub candidates: Vec<AppCandidate>,
}

impl AppLocation {
    /// 没找到时的占位
    pub fn missing(id: AppId, note: String) -> Self {
        AppLocation {
            id: id.as_str().to_string(),
            name: id.display_name().to_string(),
            description: id.description().to_string(),
            path: String::new(),
            available: false,
            source: "missing".to_string(),
            note,
            builtin_path: String::new(),
            candidates: Vec::new(),
        }
    }
}

/// 查找上下文：从哪几个目录里找
#[derive(Debug, Clone, Default)]
pub struct AppContext {
    /// 应用数据目录（程序托管的副本放在这里）
    pub data_dir: Option<std::path::PathBuf>,
    /// 打包后的资源目录
    pub resource_dir: Option<std::path::PathBuf>,
    /// 可执行文件所在目录
    pub exe_dir: Option<std::path::PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_ids_have_unique_strings() {
        let mut seen = std::collections::HashSet::new();
        for id in AppId::ALL {
            assert!(seen.insert(id.as_str()), "{} 重复", id.as_str());
        }
        assert_eq!(seen.len(), AppId::ALL.len());
    }

    #[test]
    fn from_str_roundtrip_and_case_insensitive() {
        for id in AppId::ALL {
            assert_eq!(AppId::from_str(id.as_str()), Some(*id));
            assert_eq!(AppId::from_str(&id.as_str().to_uppercase()), Some(*id));
        }
        assert_eq!(AppId::from_str("  vscode  "), Some(AppId::Vscode));
        assert_eq!(AppId::from_str("photoshop"), None);
    }

    #[test]
    fn every_id_has_search_metadata() {
        for id in AppId::ALL {
            assert!(!id.display_name().is_empty(), "{} 缺展示名", id.as_str());
            assert!(
                !id.executable_names().is_empty(),
                "{} 缺可执行文件名",
                id.as_str()
            );
            assert!(
                !id.registry_keywords().is_empty(),
                "{} 缺注册表关键词",
                id.as_str()
            );
            assert!(!id.macos_paths().is_empty(), "{} 缺 macOS 路径", id.as_str());
        }
    }

    #[test]
    fn executable_names_cover_platform_conventions() {
        // Windows 下必须列 .exe，非 Windows 下必须有一个不带后缀的名字
        assert!(AppId::Openconnect
            .executable_names()
            .iter()
            .any(|name| name.ends_with(".exe")));
        assert!(AppId::Openconnect
            .executable_names()
            .iter()
            .any(|name| !name.ends_with(".exe")));
    }

    #[test]
    fn missing_location_is_not_available() {
        let location = AppLocation::missing(AppId::Openconnect, "没找到".to_string());
        assert!(!location.available);
        assert_eq!(location.source, "missing");
        assert!(location.path.is_empty());
        // 标题必须点明这是命令行程序，免得和别的版本混在一起
        assert_eq!(location.name, "OpenConnect（命令行）");
    }

    /// openconnect 只认自己那个可执行文件，不会把图形界面版的认成自己。
    #[test]
    fn openconnect_only_accepts_its_own_executable() {
        assert_eq!(AppId::Openconnect.executable_names(), &["openconnect.exe", "openconnect"]);
        assert!(AppId::Openconnect
            .executable_names()
            .iter()
            .all(|name| !name.contains("gui")));
    }

    #[test]
    fn openconnect_cli_description_mentions_its_advantages() {
        // 唯一那条要讲清它凭什么能扛下连接
        let description = AppId::Openconnect.description();
        assert!(description.contains("自动登录"), "实际: {}", description);
        assert!(description.contains("分流"), "实际: {}", description);
    }
}
