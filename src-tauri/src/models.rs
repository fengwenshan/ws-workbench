use serde::{Deserialize, Serialize};

/// 默认排除规则
pub const DEFAULT_EXCLUDE: &[&str] = &[
    ".DS_Store",
    "*.md",
    ".git",
    ".git/**",
    ".svn",
    ".svn/**",
    ".idea",
    ".idea/**",
    ".vscode",
    ".vscode/**",
    ".claude",
    ".claude/**",
    ".trae",
    ".trae/**",
    "*.scss",
    "*.less",
    ".gitignore",
    "node_modules",
    "node_modules/**",
    "*.zip",
];

/// 各版本新增的默认排除规则：(版本号, 规则列表)
/// 版本 1 为初版规则集（无新增），版本 2 起为后续新增的规则
pub const DEFAULT_EXCLUDE_ADDITIONS: &[(u32, &[&str])] = &[(2, &[".*", ".*/**"])];

/// 当前默认排除规则版本号，新增默认规则时递增
pub const DEFAULT_EXCLUDE_VERSION: u32 = 2;

/// 完整的默认排除规则 = 初版规则 + 各版本新增规则
pub fn default_exclude_rules() -> Vec<String> {
    let mut rules: Vec<String> = DEFAULT_EXCLUDE.iter().map(|s| s.to_string()).collect();
    for (version, additions) in DEFAULT_EXCLUDE_ADDITIONS {
        if *version <= DEFAULT_EXCLUDE_VERSION {
            rules.extend(additions.iter().map(|s| s.to_string()));
        }
    }
    rules
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub groups: Vec<ProjectGroup>,
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    #[serde(default)]
    pub default_exclude: Vec<String>,
    #[serde(default)]
    pub default_exclude_version: u32,
    /// VPN 配置列表
    #[serde(default)]
    pub vpn_profiles: Vec<crate::vpn::models::VpnProfile>,
    /// VPN 规则集列表
    #[serde(default)]
    pub vpn_rule_sets: Vec<crate::vpn::models::VpnRuleSet>,
    /// 上次选中的 VPN 配置
    #[serde(default)]
    pub vpn_active_profile_id: String,
    /// 用户在设置页手动指定的应用可执行文件路径（Windows 专有设置）
    #[serde(default)]
    pub app_paths: Vec<crate::apps::models::AppPathOverride>,
    /// 注册免 UAC 计划任务时用的会话目录。
    ///
    /// 计划的 `/TR` 把会话目录路径写死了，会话目录一旦回退（用户目录被
    /// 受控文件夹访问/企业策略挡住），旧任务就作废了。这里记一份，
    /// 连接时和当前目录比一下即可，不必每次都去查任务定义。
    #[serde(default)]
    pub vpn_worker_session_dir: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            groups: vec![],
            projects: vec![],
            default_exclude: default_exclude_rules(),
            default_exclude_version: DEFAULT_EXCLUDE_VERSION,
            vpn_profiles: vec![],
            vpn_rule_sets: vec![],
            vpn_active_profile_id: String::new(),
            app_paths: vec![],
            vpn_worker_session_dir: String::new(),
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
    pub exclude: Vec<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
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
