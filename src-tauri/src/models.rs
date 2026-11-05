use serde::{Deserialize, Serialize};

/// 打包包含规则：**命中任一规则的文件才进包**（白名单）。
///
/// 面向 layui 工程的打包白名单：只放浏览器/控件运行时真的会请求的类型。
/// `scss`/`ts`/`map`/`md`/设计源文件、构建配置这些开发期产物不在列表里；
/// `.thm`/`.tjs`/`.dotm` 是 layui 模板与 WPS 控件，`.doc`/`.docx` 是 WebOffice
/// 新建正文的空白模板（旧版 Word 用 `.doc`），都按运行时资源处理。
/// 路径里以 `.` 或 `_` 开头的段一律跳过，由 `packer::scan_files` 硬编码处理。
pub const DEFAULT_INCLUDE: &[&str] = &[
    "*.html",
    "*.htm",
    "*.css",
    "*.js",
    "*.mjs",
    "*.json",
    "*.png",
    "*.jpg",
    "*.jpeg",
    "*.gif",
    "*.svg",
    "*.webp",
    "*.avif",
    "*.ico",
    "*.bmp",
    "*.woff",
    "*.woff2",
    "*.ttf",
    "*.eot",
    "*.otf",
    "*.mp4",
    "*.webm",
    "*.mp3",
    "*.wav",
    "*.ogg",
    "*.txt",
    "*.xml",
    // layui 工程特有：模板与 WPS 控件不是开发期产物，运行时真的会被请求
    // - .thm: layui 表单/列表模板（rsViewRender.js 按 APP_ROOT + '/xmodules/pcp/tpl/**.thm' 拉取）
    // - .tjs: 模板配套脚本
    // - .dotm: WPS 控件模板（<object data=".../Normal.dotm">）
    "*.thm",
    "*.tjs",
    "*.dotm",
    // WebOffice 新建正文的空白模板：rsWebOffice.js 用 $.ajax 直接 GET newDocument.doc(x)
    // - .docx: 新版本 Office 模板
    // - .doc: 旧版 Word 模板，部分 layui 工程只放了这一份
    "*.docx",
    "*.doc",
];

/// 当前包含规则版本号。以后新增默认规则时递增，老配置由
/// [`migrate_default_include`] 补齐（只追加缺的，不动用户已有规则）。
pub const DEFAULT_INCLUDE_VERSION: u32 = 4;

/// 默认包含规则列表
pub fn default_include_rules() -> Vec<String> {
    DEFAULT_INCLUDE.iter().map(|rule| rule.to_string()).collect()
}

/// 老配置补齐当前版本的默认包含规则。
///
/// 返回是否发生变化。规则为空（老版本没有包含规则字段）或版本落后时，
/// 把缺的默认规则追加进去；用户自己加/删的规则不会被覆盖。
pub fn migrate_default_include(rules: &mut Vec<String>, version: &mut u32) -> bool {
    if *version >= DEFAULT_INCLUDE_VERSION {
        return false;
    }
    for rule in DEFAULT_INCLUDE {
        if !rules.iter().any(|existing| existing == rule) {
            rules.push((*rule).to_string());
        }
    }
    *version = DEFAULT_INCLUDE_VERSION;
    true
}

/// 附件元信息。`id` 即磁盘上密文文件的主名（`<id>.wst`）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub id: String,
    pub name: String,
    pub size: u64,
    #[serde(default)]
    pub ext: String,
    #[serde(default)]
    pub created_at: i64,
    /// 该附件实际落盘目录（快照上传时的解析结果，便于删除/导出）
    #[serde(default)]
    pub dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    /// 是否为默认分组（默认分组不允许删除，但内部项目可删除）
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub created_at: String,
    /// 加密笔记（Markdown）
    #[serde(default)]
    pub note: String,
    /// 附件列表
    #[serde(default)]
    pub attachments: Vec<AttachmentMeta>,
}

/// 只随 config.json 保存的应用设置；业务数据改由 SQLite 承载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub default_include: Vec<String>,
    #[serde(default)]
    pub default_include_version: u32,
    /// 上次选中的 VPN 配置
    #[serde(default)]
    pub vpn_active_profile_id: String,
    /// 注册免 UAC 计划任务时用的会话目录
    #[serde(default)]
    pub vpn_worker_session_dir: String,
    /// 全局默认附件上传目录（分组/项目未单独设置时使用）
    #[serde(default)]
    pub default_upload_dir: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            default_include: default_include_rules(),
            default_include_version: DEFAULT_INCLUDE_VERSION,
            vpn_active_profile_id: String::new(),
            vpn_worker_session_dir: String::new(),
            default_upload_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub groups: Vec<ProjectGroup>,
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    #[serde(default)]
    pub default_include: Vec<String>,
    #[serde(default)]
    pub default_include_version: u32,
    /// VPN 配置列表
    #[serde(default)]
    pub vpn_profiles: Vec<crate::vpn::models::VpnProfile>,
    /// VPN 规则集列表
    #[serde(default)]
    pub vpn_rule_sets: Vec<crate::vpn::models::VpnRuleSet>,
    /// 上次选中的 VPN 配置
    #[serde(default)]
    pub vpn_active_profile_id: String,
    /// 注册免 UAC 计划任务时用的会话目录。
    ///
    /// 计划的 `/TR` 把会话目录路径写死了，会话目录一旦回退（用户目录被
    /// 受控文件夹访问/企业策略挡住），旧任务就作废了。这里记一份，
    /// 连接时和当前目录比一下即可，不必每次都去查任务定义。
    #[serde(default)]
    pub vpn_worker_session_dir: String,
    /// 全局默认附件上传目录（分组/项目未单独设置时使用）
    #[serde(default)]
    pub default_upload_dir: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            groups: vec![],
            projects: vec![],
            default_include: default_include_rules(),
            default_include_version: DEFAULT_INCLUDE_VERSION,
            vpn_profiles: vec![],
            vpn_rule_sets: vec![],
            vpn_active_profile_id: String::new(),
            vpn_worker_session_dir: String::new(),
            default_upload_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub id: String,
    pub name: String,
    pub source_dir: String,
    pub output_dir: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    /// 加密笔记（Markdown）
    #[serde(default)]
    pub note: String,
    /// 附件列表
    #[serde(default)]
    pub attachments: Vec<AttachmentMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackResult {
    pub success: bool,
    pub total_files: u64,
    pub copied_files: u64,
    pub skipped_files: u64,
    pub elapsed_ms: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackProgress {
    pub phase: String,
    pub current: u64,
    pub total: u64,
    pub current_file: String,
    pub percentage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectValidation {
    pub valid: bool,
    pub source_exists: bool,
    pub file_count: u64,
    pub warnings: Vec<String>,
}

/// 项目类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectType {
    Vue2,
    Vue3,
    React,
    Layui,
    SpringBoot,
    Spring,
    Struts2,
    Java,
    Unknown,
}

/// 版本控制类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum VcsType {
    Git,
    Svn,
    None,
}

/// 源目录版本控制信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VcsInfo {
    pub vcs_type: VcsType,
    /// 远程仓库地址，未检测到则为空字符串
    #[serde(default)]
    pub url: String,
}


#[cfg(test)]
mod default_include_tests {
    use super::*;

    /// 默认包含集 = 浏览器能直接用的类型；开发期产物一个都不能进
    #[test]
    fn default_rules_are_browser_loadable_types() {
        let rules = default_include_rules();
        let has = |rule: &str| rules.iter().any(|item| item == rule);
        for rule in [
            "*.html", "*.htm", "*.css", "*.js", "*.json", "*.png", "*.jpg", "*.svg",
            "*.woff2", "*.ttf", "*.mp4", "*.txt",
            // layui 工程特有
            "*.thm", "*.tjs", "*.dotm", "*.docx", "*.doc",
        ] {
            assert!(has(rule), "默认包含规则缺少 {}", rule);
        }
        for dev_only in [
            "*.scss", "*.less", "*.ts", "*.tsx", "*.vue", "*.map", "*.md", "*.psd",
            "*.pdf", "*.log", "package.json", "vite.config.*", "*.zip", "node_modules",
        ] {
            assert!(!has(dev_only), "开发期文件不该默认进包: {}", dev_only);
        }
    }

    #[test]
    fn default_rules_have_no_duplicates() {
        let rules = default_include_rules();
        let mut sorted = rules.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(rules.len(), sorted.len(), "默认包含规则里有重复项");
    }

    /// 老配置（没有 include 字段 / 版本 0）迁移后拿到完整默认集
    #[test]
    fn migration_from_legacy_config_fills_defaults() {
        let mut rules: Vec<String> = Vec::new();
        let mut version = 0;
        assert!(migrate_default_include(&mut rules, &mut version));
        assert_eq!(version, DEFAULT_INCLUDE_VERSION);
        let mut sorted = rules.clone();
        sorted.sort();
        let mut expected = default_include_rules();
        expected.sort();
        assert_eq!(sorted, expected);

        // 再跑不重复、不变化
        let before = rules.len();
        assert!(!migrate_default_include(&mut rules, &mut version));
        assert_eq!(rules.len(), before);
    }

    /// 用户自己删掉的规则不会被重新塞回去（版本已是最新）
    #[test]
    fn migration_respects_user_edits_after_upgrade() {
        let mut rules = vec!["*.html".to_string(), "*.js".to_string()];
        let mut version = DEFAULT_INCLUDE_VERSION;
        assert!(!migrate_default_include(&mut rules, &mut version));
        assert_eq!(rules, vec!["*.html", "*.js"]);
    }
}
