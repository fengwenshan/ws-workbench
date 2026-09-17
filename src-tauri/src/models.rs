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
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            groups: vec![],
            projects: vec![],
            default_exclude: default_exclude_rules(),
            default_exclude_version: DEFAULT_EXCLUDE_VERSION,
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
    Layui,
    Vue,
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
