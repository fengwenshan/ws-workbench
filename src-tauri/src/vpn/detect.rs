//! VPN 连接器探测
//!
//! 可执行文件的**查找**已经统一交给 [`crate::apps`]，本模块只负责
//! 「拿到候选路径之后，怎么判断它能不能用」：
//!
//! - openconnect 要真正执行一次 `--version`，因为 Wintun 版解压后缺 DLL 是很常见的情况，
//!   光看文件存在会把一个跑不起来的二进制当成可用。
//!
//! Windows 只探测安装包内置的 openconnect，不碰注册表 / 常见目录 / PATH；
//! macOS 的顺序是「手动指定 > 程序托管副本 > 内置资源 > Homebrew / 常见目录 > PATH」，
//! 由 `apps::locate` 定义，这里不重复实现。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::apps::{self, AppContext, AppId};
use crate::vpn::models::{ConnectorInfo, ConnectorKind};

/// 探测 openconnect 时的超时。正常几十毫秒，给足余量避免慢机器误判。
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// 从 `openconnect --version` 输出里取版本号
fn parse_openconnect_version(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("OpenConnect version ") {
            return rest.trim().to_string();
        }
    }
    String::new()
}

/// 候选来源到展示名的后缀
fn source_suffix(source: &str) -> &'static str {
    match source {
        "custom" => "（手动指定）",
        "managed" => "（已指定）",
        "bundled" => "（内置）",
        _ => "",
    }
}

/// 启动并按版本号判断一个 openconnect 是否真的能用
fn probe_openconnect(path: &Path, source: &str) -> ConnectorInfo {
    let (text, ok) = crate::process::run_with_timeout(path, &["--version"], PROBE_TIMEOUT);
    let version = parse_openconnect_version(&text);
    let available = ok && !version.is_empty();
    let note = if available {
        String::new()
    } else if text.contains("执行超时") {
        "执行超时，可能是缺少依赖 DLL".to_string()
    } else {
        format!(
            "无法执行: {}",
            text.trim().chars().take(160).collect::<String>()
        )
    };
    ConnectorInfo {
        kind: "openconnect".to_string(),
        name: format!("OpenConnect{}", source_suffix(source)),
        path: path.to_string_lossy().to_string(),
        version,
        available,
        source: source.to_string(),
        note,
    }
}

/// 探测 openconnect 的全部候选（仅 macOS 仍走「系统安装」这条路）
#[cfg(target_os = "macos")]
fn probe_app(id: AppId, ctx: &AppContext, override_path: &str) -> Vec<ConnectorInfo> {
    let location = apps::locate(id, ctx, override_path);
    let kind = "openconnect";

    let probe = |path: &Path, source: &str| probe_openconnect(path, source);

    let mut out: Vec<ConnectorInfo> = Vec::new();
    let mut probed: Vec<String> = Vec::new();

    // 按优先级顺序探测真实存在的候选，第一个可用的就停
    for candidate in location.candidates.iter().filter(|item| item.exists) {
        let info = probe(Path::new(&candidate.path), &candidate.source);
        let available = info.available;
        probed.push(candidate.path.clone());
        out.push(info);
        if available {
            break;
        }
    }

    // 系统里已经有能跑的 openconnect 了：之前「指定位置」登记的那份没有存在意义，删掉。
    //
    // 留着不只是占地方 —— 系统那份哪天被卸载/跑不起来，它会悄悄顶上来，用户会以为
    // 还在用自定义前缀。按产品定的规则：系统没了就回到安装引导，让人重新指定。
    if out
        .iter()
        .any(|item| item.available && item.source == "system")
    {
        if let Some(data_dir) = ctx.data_dir.as_ref() {
            let _ = apps::finder::remove_managed(AppId::Openconnect, data_dir);
        }
    }

    // 内置副本不在 candidates 里 —— `apps::locate` 把它单独放进 builtin_path，
    // 免得界面上看着像还在搜用户自己的安装目录。所以候选全部不可用时必须补探它，
    // 否则内置 openconnect 永远被判成 missing，「自动」模式会报「没有可用连接器」。
    if !out.iter().any(|item| item.available) {
        let builtin = location.builtin_path.trim();
        if !builtin.is_empty() && !probed.iter().any(|path| path == builtin) {
            out.push(probe(Path::new(builtin), "bundled"));
        }
    }

    // 手动指定但文件不在：明确报出来，否则用户不知道设置没生效
    if let Some(custom) = location
        .candidates
        .iter()
        .find(|item| item.source == "custom" && !item.exists)
    {
        out.push(ConnectorInfo {
            kind: kind.to_string(),
            name: format!("{}（手动指定）", id.display_name()),
            path: custom.path.clone(),
            version: String::new(),
            available: false,
            source: "custom".to_string(),
            note: "设置里指定的路径不存在，请到设置页检查".to_string(),
        });
    }

    if !out.iter().any(|item| item.available) {
        let note = if location.note.trim().is_empty() {
            "没有找到可执行文件".to_string()
        } else {
            location.note.clone()
        };
        out.insert(
            0,
            ConnectorInfo {
                kind: kind.to_string(),
                name: id.display_name().to_string(),
                path: String::new(),
                version: String::new(),
                available: false,
                source: "missing".to_string(),
                note,
            },
        );
    }

    out
}

/// 探测连接器候选
///
/// Windows 只认安装包内置的 openconnect：不去注册表、常见安装目录或 PATH 里
/// 找第二份，避免「本机装过别的版本」把连接行为带偏。macOS 没有随包分发
/// 可执行文件，仍沿用系统查找。
pub fn detect_all(ctx: &AppContext, custom_openconnect: &str) -> Vec<ConnectorInfo> {
    #[cfg(target_os = "windows")]
    {
        let _ = custom_openconnect;
        match apps::bundled_binary(AppId::Openconnect, ctx) {
            Some(path) => vec![probe_openconnect(&path, "bundled")],
            None => vec![ConnectorInfo {
                kind: "openconnect".to_string(),
                name: "OpenConnect（内置）".to_string(),
                path: String::new(),
                version: String::new(),
                available: false,
                source: "missing".to_string(),
                note: "安装包缺少内置 openconnect：resources/apps/openconnect/openconnect.exe"
                    .to_string(),
            }],
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        probe_app(AppId::Openconnect, ctx, custom_openconnect)
    }
}

/// 按用户指定的连接器偏好，从探测结果里挑一个可用的
///
/// 现在只剩 openconnect 一条通道，`auto` 与 `openconnect` 的挑选结果完全一致；
/// 保留 `want` 是为了让枚举匹配穷尽，将来再加通道时编译器会强制这里处理。
pub fn pick_connector(
    candidates: &[ConnectorInfo],
    want: ConnectorKind,
) -> Option<&ConnectorInfo> {
    match want {
        ConnectorKind::Auto | ConnectorKind::Openconnect => candidates
            .iter()
            .find(|item| item.kind == "openconnect" && item.available && !item.path.is_empty()),
    }
}

/// 懒加载连接器版本号
pub fn fetch_version(info: &ConnectorInfo) -> String {
    if !info.version.is_empty() || !info.available || info.path.is_empty() {
        return info.version.clone();
    }
    let path = PathBuf::from(&info.path);
    let (text, _) = crate::process::run_with_timeout(&path, &["--version"], PROBE_TIMEOUT);
    parse_openconnect_version(&text)
}

/// 登记结果：版本号 + 实际记住的 openconnect 位置。
///
/// 位置要回显到界面上 —— 用户专门强调过「hb 是自定义目录，名字可能是别的」，
/// 只回一句「已记住」等于没说，得让他看见程序到底认的是哪个文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedOpenconnect {
    pub version: String,
    /// 真正被记住的可执行文件（Cellar 版本目录会折算成稳定的 `prefix/bin`）
    pub path: String,
}

/// 把用户指定的 openconnect「登记」成程序托管的连接器，返回版本号与位置。
///
/// Windows 上只认安装包内置的那份，不允许登记外部副本。
#[cfg(windows)]
pub fn import_openconnect(_data_dir: &Path, _source_dir: &Path) -> Result<ImportedOpenconnect, String> {
    Err("Windows 版只使用安装包内置的 openconnect，不支持导入外部副本".to_string())
}

/// macOS/Linux：把用户指定的 openconnect 登记到托管目录，返回版本号与位置。
///
/// **不复制二进制**。这里先后踩过两个坑：
/// - Homebrew 的 openconnect 通过绝对路径加载自己前缀里的动态库，拷出来的副本只是
///   碰巧能跑，`brew upgrade` 之后依赖失配；
/// - 复制要先读用户目录里的文件，自定义前缀很容易被权限/隐私策略挡住，现场报的就是
///   `复制 openconnect 失败: Permission denied (os error 13)`。
///
/// 所以改成「登记软链」：只写不读，源文件原样保留，brew 升级也跟着走。
/// 用户的选法不固定（可执行文件 / bin 目录 / Cellar 版本目录），交给 `resolve_binary` 统一解析。
#[cfg(not(windows))]
pub fn import_openconnect(data_dir: &Path, source: &Path) -> Result<ImportedOpenconnect, String> {
    let picked = apps::finder::resolve_binary(AppId::Openconnect, source)?;
    // Cellar 的版本目录会被 brew 升级换掉，能改指 prefix/bin 就改
    let binary = apps::finder::stable_binary_path(&picked);

    // 先探测：跑不起来就把原始输出带回去，别让用户猜
    let (text, ok) = crate::process::run_with_timeout(&binary, &["--version"], PROBE_TIMEOUT);
    let version = parse_openconnect_version(&text);
    if !ok || version.is_empty() {
        return Err(format!(
            "这份 openconnect 跑不起来（缺少依赖库或没有执行权限）: {}",
            text.trim().chars().take(200).collect::<String>()
        ));
    }

    let linked = apps::finder::link_binary(AppId::Openconnect, &binary, data_dir)?;

    // 登记后再验一次软链本身：断链、权限问题要在这里暴露，不能留个坏副本给探测用
    let (verify_text, verify_ok) =
        crate::process::run_with_timeout(&linked, &["--version"], PROBE_TIMEOUT);
    if !verify_ok || parse_openconnect_version(&verify_text).is_empty() {
        let _ = std::fs::remove_file(&linked);
        return Err(format!(
            "登记后校验失败，已撤销: {}",
            verify_text.trim().chars().take(200).collect::<String>()
        ));
    }
    Ok(ImportedOpenconnect {
        version,
        path: binary.to_string_lossy().to_string(),
    })
}

/// 删除程序托管的 openconnect
pub fn remove_managed_openconnect(data_dir: &Path) -> Result<(), String> {
    apps::finder::remove_managed(AppId::Openconnect, data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(kind: &str, available: bool, path: &str) -> ConnectorInfo {
        ConnectorInfo {
            kind: kind.to_string(),
            name: kind.to_string(),
            path: path.to_string(),
            version: String::new(),
            available,
            source: "system".to_string(),
            note: String::new(),
        }
    }

    #[test]
    fn parse_openconnect_version_line() {
        let text = "OpenConnect version v9.12\nUsing GnuTLS 3.8.3\n";
        assert_eq!(parse_openconnect_version(text), "v9.12");
    }

    #[test]
    fn parse_openconnect_version_missing() {
        assert_eq!(parse_openconnect_version("whatever"), "");
    }

    #[test]
    fn source_suffix_labels_are_distinct() {
        assert_eq!(source_suffix("custom"), "（手动指定）");
        assert_eq!(source_suffix("managed"), "（已指定）");
        assert_eq!(source_suffix("bundled"), "（内置）");
        assert_eq!(source_suffix("system"), "");
        assert_eq!(source_suffix("path"), "");
    }

    #[test]
    fn picks_openconnect_for_both_preferences() {
        let list = vec![info("openconnect", true, "x")];
        assert_eq!(
            pick_connector(&list, ConnectorKind::Auto).unwrap().kind,
            "openconnect"
        );
        assert_eq!(
            pick_connector(&list, ConnectorKind::Openconnect).unwrap().kind,
            "openconnect"
        );
    }

    #[test]
    fn returns_none_when_nothing_usable() {
        let list = vec![info("openconnect", false, "")];
        assert!(pick_connector(&list, ConnectorKind::Auto).is_none());
        assert!(pick_connector(&list, ConnectorKind::Openconnect).is_none());
    }

    #[test]
    fn unavailable_candidate_with_path_is_not_picked() {
        // 探测失败但路径还在（例如缺 DLL），不能被当成可用
        let list = vec![info("openconnect", false, r"C:\app\openconnect.exe")];
        assert!(pick_connector(&list, ConnectorKind::Auto).is_none());
        // 可用但路径为空（missing 占位）同样不能选
        let list = vec![info("openconnect", true, "")];
        assert!(pick_connector(&list, ConnectorKind::Auto).is_none());
    }

    #[test]
    fn detect_all_returns_openconnect_channel() {
        // 空上下文下也要有结果（哪怕是 missing 占位），界面才能明确告诉用户「没找到」
        let ctx = AppContext::default();
        let found = detect_all(&ctx, "");
        assert!(found.iter().any(|item| item.kind == "openconnect"));
        // 只有一条通道，不该再冒出别的 kind
        assert!(found.iter().all(|item| item.kind == "openconnect"));
    }

    // 内置 openconnect 只随 Windows 包分发、也只有 Windows 的 detect_all 会探测
    // bundled；macOS 的 resources 目录里同样带着这些文件，用例若不加平台限制会误报。
    #[cfg(target_os = "windows")]
    #[test]
    fn bundled_openconnect_is_probed_not_skipped() {
        // 内置副本默认不在 candidates 里，必须单独探测；否则「自动」模式会直接报
        // 「没有可用连接器」—— 用户现场就是这么发生的。
        let ctx = AppContext::default();
        let builtin = apps::bundled_binary(AppId::Openconnect, &ctx);
        if builtin.is_none() {
            // 内置二进制体积大，不进版本库；本地没放就跳过，别把测试搞成环境依赖
            return;
        }

        let probed = detect_all(&ctx, "");
        assert!(
            probed.iter().any(|item| item.source == "bundled"),
            "内置 openconnect 没有被探测，实际结果: {:?}",
            probed
                .iter()
                .map(|item| format!("{}:{}", item.kind, item.source))
                .collect::<Vec<_>>()
        );

        // 真正的行为断言：内置副本装好了，「自动」就该选它
        let picked = pick_connector(&probed, ConnectorKind::Auto).expect("至少要能选出一个连接器");
        assert_eq!(picked.source, "bundled");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn missing_custom_path_is_reported_for_openconnect() {
        let ctx = AppContext::default();
        let found = detect_all(&ctx, r"Z:\nope\openconnect.exe");
        let custom = found
            .iter()
            .find(|item| item.source == "custom")
            .expect("应该保留手动指定的记录");
        assert!(!custom.available);
        assert!(custom.note.contains("不存在"), "实际: {}", custom.note);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_never_falls_back_to_external_openconnect() {
        // 手动指定一份不存在的路径时，Windows 也只能返回内置副本/缺失占位，
        // 不能把系统里装的 openconnect 混进候选。
        let ctx = AppContext::default();
        let found = detect_all(&ctx, r"Z:\nope\openconnect.exe");
        assert!(found.iter().all(|item| item.source != "custom"));
        assert!(found.iter().all(|item| item.source == "bundled" || item.source == "missing"));
    }

    #[test]
    fn import_rejects_directory_without_binary() {
        let source = std::env::temp_dir().join(format!("ws-oc-empty-{}", std::process::id()));
        let data = std::env::temp_dir().join(format!("ws-oc-data-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&source);
        let _ = std::fs::create_dir_all(&data);

        let error = import_openconnect(&data, &source).unwrap_err();
        #[cfg(windows)]
        assert!(error.contains("只使用安装包内置"), "实际: {}", error);
        #[cfg(not(windows))]
        assert!(error.contains("没有找到"), "实际: {}", error);

        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&data);
    }

    /// 造一个能跑 `--version` 的假 openconnect（真跑一次，不依赖本机装没装）
    #[cfg(unix)]
    fn fake_openconnect(dir: &Path, version: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("openconnect");
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho 'OpenConnect version {}'\n", version),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ws-oc-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// 核心行为：macOS 上「导入」是登记路径（软链），不是复制二进制
    #[cfg(unix)]
    #[test]
    fn import_registers_path_instead_of_copying() {
        let root = temp_root("import-link");
        let source = fake_openconnect(&root.join("hb/bin"), "v9.99");
        let data = root.join("data");

        let imported = import_openconnect(&data, &source).unwrap();
        assert_eq!(imported.version, "v9.99");
        assert_eq!(
            imported.path,
            source.to_string_lossy().to_string(),
            "要回显记住的位置"
        );

        let managed = apps::managed_dir(AppId::Openconnect, &data).join("openconnect");
        let meta = std::fs::symlink_metadata(&managed).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "托管副本必须是软链；复制二进制既多余又会撞权限（现场 Permission denied）"
        );
        assert_eq!(std::fs::read_link(&managed).unwrap(), source);
        assert!(source.is_file(), "源文件必须原样保留");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 用户截图里框的是 `Cellar/openconnect/9.21`，这一层也必须能导入，
    /// 并且软链要指到稳定的 `prefix/bin`（否则 brew 升级后断链）
    #[cfg(unix)]
    #[test]
    fn import_accepts_homebrew_version_dir() {
        let root = temp_root("import-cellar");
        let prefix = root.join("hb");
        let real = fake_openconnect(&prefix.join("Cellar/openconnect/9.21/bin"), "v9.21");
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::os::unix::fs::symlink(&real, prefix.join("bin/openconnect")).unwrap();
        let data = root.join("data");

        let imported =
            import_openconnect(&data, &prefix.join("Cellar/openconnect/9.21")).unwrap();
        assert_eq!(imported.version, "v9.21");
        assert_eq!(
            imported.path,
            prefix
                .join("bin/openconnect")
                .to_string_lossy()
                .to_string(),
            "回显的位置要是稳定的 prefix/bin，不是带版本号的 Cellar 路径"
        );

        let managed = apps::managed_dir(AppId::Openconnect, &data).join("openconnect");
        assert_eq!(
            std::fs::read_link(&managed).unwrap(),
            prefix.join("bin/openconnect"),
            "软链要指向稳定的 prefix/bin，不能钉死带版本号的 Cellar 路径"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 跑不起来的二进制要当场报错，并且不能留下半截托管副本
    #[cfg(unix)]
    #[test]
    fn import_reports_unrunnable_binary_and_leaves_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("import-broken");
        let dir = root.join("hb/bin");
        std::fs::create_dir_all(&dir).unwrap();
        let broken = dir.join("openconnect");
        std::fs::write(&broken, "#!/bin/sh\necho 'dyld: missing library' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
        let data = root.join("data");

        let error = import_openconnect(&data, &broken).unwrap_err();
        assert!(error.contains("跑不起来"), "实际: {}", error);
        assert!(
            !apps::managed_dir(AppId::Openconnect, &data)
                .join("openconnect")
                .exists(),
            "失败时不能留下坏副本"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 端到端：先前「指定位置」登记过一份，后来 openconnect 换到了标准路径
    ///
    /// 点「检测」必须落到系统那份（`source == "system"`），登记的那份不再参与；
    /// 界面上「已指定 openconnect + 路径」那行就是靠这个结果消失的。
    #[cfg(unix)]
    #[test]
    fn detection_prefers_standard_install_over_registration() {
        // 标准安装位置：本机没有就跳过（CI 上大概率没装），有但要先确认它真能跑
        let standard = ["/opt/homebrew/bin/openconnect", "/usr/local/bin/openconnect", "/opt/local/bin/openconnect"]
            .iter()
            .map(PathBuf::from)
            .find(|path| path.is_file());
        let standard = match standard {
            Some(path) => path,
            None => return,
        };
        let (text, ok) = crate::process::run_with_timeout(&standard, &["--version"], PROBE_TIMEOUT);
        if !ok || parse_openconnect_version(&text).is_empty() {
            return;
        }

        let root = temp_root("detect-system-wins");
        let data = root.join("data");
        // 模拟「指定位置」登记：托管目录里有个指向自定义前缀的软链
        let custom = fake_openconnect(&root.join("hb/bin"), "v0.0.1-custom");
        let managed = apps::managed_dir(AppId::Openconnect, &data);
        std::fs::create_dir_all(&managed).unwrap();
        std::os::unix::fs::symlink(&custom, managed.join("openconnect")).unwrap();

        let ctx = AppContext {
            data_dir: Some(data),
            ..Default::default()
        };
        let found = detect_all(&ctx, "");
        let available: Vec<&ConnectorInfo> = found.iter().filter(|item| item.available).collect();
        assert_eq!(
            available.len(),
            1,
            "检测到第一份能用的就停，不该同时留着两份: {:?}",
            found
        );
        assert_eq!(
            available[0].source, "system",
            "标准路径里有能跑的 openconnect 时，不再用「指定位置」登记的那份"
        );
        assert_eq!(available[0].path, standard.to_string_lossy().to_string());
        assert!(!available[0].version.is_empty(), "要带上探测到的版本号");
        assert!(
            !managed.join("openconnect").exists(),
            "登记的那份要被删掉：系统没了就回安装引导重新指定，不能偷偷顶着用"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
