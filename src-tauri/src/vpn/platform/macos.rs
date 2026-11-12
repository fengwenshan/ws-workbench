//! VPN 连接器平台层：macOS 实现
//!
//! 这个文件只在 macOS 目标下编译，因此内部不再写 `#[cfg(target_os = "macos")]`。
//!
//! macOS 的模型是「向外找系统安装的那份 openconnect」：
//! - 候选来源：手动指定 > Homebrew / 常见安装目录 > PATH（由 [`crate::apps`] 统一收集），
//!   外加「选择安装目录」在数据目录里记住的一行路径；
//! - 安装包里的 `resources/apps/openconnect/openconnect.exe` 是 Windows 可执行文件，
//!   在 mac 上跑不起来，所以**永远不探测内置副本**；
//! - 导入只记路径（`<data>/vpn/openconnect-path.txt`），不复制二进制、不建软链；
//! - 旧版本留下的副本/软链在探测与移除时顺手清掉。

use std::path::{Path, PathBuf};

use crate::apps::{self, AppCandidate, AppContext, AppId, AppLocation};
use crate::vpn::models::ConnectorInfo;

use super::common::{
    describe_launch_failure, parse_openconnect_version, probe_openconnect, ImportedOpenconnect,
    PROBE_TIMEOUT,
};

/// 「选择安装目录」记住的 openconnect 位置：数据目录里只写一行路径。
///
/// 不复制二进制、也不建软链：
/// - 复制件在 `brew upgrade` 之后会变成过期副本，而且数据目录里出现可执行文件，
///   很容易被误认成程序自带的副本（现场已经把它当成「Windows 内置文件」误判过）；
/// - 软链路径同样会让人以为程序在用自己的副本。
///
/// 只记路径，执行时直接用原文件；旧版本留下的复制件/软链由
/// [`cleanup_legacy_registration`] 清理。
fn stored_connector_path_file(data_dir: &Path) -> PathBuf {
    data_dir.join("vpn").join("openconnect-path.txt")
}

fn read_stored_connector_path(data_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(stored_connector_path_file(data_dir)).ok()?;
    let path = PathBuf::from(text.trim());
    path.is_file().then_some(path)
}

fn store_connector_path(data_dir: &Path, binary: &Path) -> Result<(), String> {
    let file = stored_connector_path_file(data_dir);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建连接器配置目录失败 {}: {}", parent.display(), e))?;
    }
    std::fs::write(&file, format!("{}\n", binary.display()))
        .map_err(|e| format!("写入连接器路径失败 {}: {}", file.display(), e))
}

fn clear_stored_connector_path(data_dir: &Path) {
    let _ = std::fs::remove_file(stored_connector_path_file(data_dir));
}

/// 清理旧版本登记方式留下的副本/软链。
///
/// 旧版本把 openconnect 复制或软链到 `<data>/apps/openconnect/`（1.0.25 起迁到
/// `<data>/connectors/openconnect/`）。复制件在 brew 升级后就过期了，软链也没必要
/// 再留着：`recover = true` 时把软链的真实目标转成路径记录，两个目录一律删掉。
/// 如果用户真的只靠那份副本，清理后界面会回到安装引导，重新选一次目录即可。
fn cleanup_legacy_registration(data_dir: &Path, recover: bool) {
    let dirs = [
        apps::finder::managed_dir(AppId::Openconnect, data_dir),
        data_dir.join(apps::finder::APPS_DIR).join("openconnect"),
    ];
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        if recover && read_stored_connector_path(data_dir).is_none() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Ok(meta) = std::fs::symlink_metadata(&path) else {
                        continue;
                    };
                    if !meta.file_type().is_symlink() {
                        continue;
                    }
                    let Ok(target) = std::fs::read_link(&path) else {
                        continue;
                    };
                    let target = if target.is_absolute() {
                        target
                    } else {
                        dir.join(target)
                    };
                    if target.is_file() {
                        let _ = store_connector_path(data_dir, &target);
                        break;
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 探测 openconnect 的全部候选：macOS 只向外找（专用路径 / 标准前缀 / 记住的路径 / PATH）。
///
/// 安装包里没有可执行的 openconnect（只有 Windows 的 exe），所以这里也不探测内置资源，
/// 没找到就进安装引导，让用户自己选目录。
fn probe_app(id: AppId, ctx: &AppContext, override_path: &str) -> Vec<ConnectorInfo> {
    if let Some(data_dir) = ctx.data_dir.as_ref() {
        cleanup_legacy_registration(data_dir, true);
    }
    let mut location = apps::locate(id, ctx, override_path);
    // 没有单独指定路径时，把「选择安装目录」记住的位置作为 managed 候选加进来：
    // 它的优先级低于系统安装位置（系统里有能跑的就不用记住的这份，和以前一致）。
    if override_path.trim().is_empty() {
        if let Some(stored) = ctx
            .data_dir
            .as_ref()
            .and_then(|data_dir| read_stored_connector_path(data_dir))
        {
            location.candidates.push(AppCandidate {
                path: stored.to_string_lossy().to_string(),
                source: "managed".to_string(),
                exists: true,
            });
            location
                .candidates
                .sort_by_key(|candidate| apps::finder::source_priority(&candidate.source));
        }
    }
    probe_location(id, ctx, location)
}

/// 把「定位结果」转成「探测结果」。
///
/// 只探 `candidates`（真正向外找到的路径），**不探 `builtin_path`**：包里的内置
/// openconnect 只有 Windows 版，在 macOS 上执行必然失败，探测它只会得到误导性的
/// 「跑不起来（缺少依赖库或没有执行权限）」，而不是干净的「没找到，请选择目录」。
fn probe_location(id: AppId, ctx: &AppContext, location: AppLocation) -> Vec<ConnectorInfo> {
    let kind = "openconnect";

    let mut out: Vec<ConnectorInfo> = Vec::new();

    // 按优先级顺序探测真实存在的候选，第一个可用的就停
    for candidate in location.candidates.iter().filter(|item| item.exists) {
        let info = probe_openconnect(Path::new(&candidate.path), &candidate.source);
        let available = info.available;
        out.push(info);
        if available {
            break;
        }
    }

    // 系统里已经有能跑的 openconnect 了：之前「指定位置」记住的那份没有存在意义，删掉。
    //
    // 留着不只是占地方 —— 系统那份哪天被卸载/跑不起来，它会悄悄顶上来，用户会以为
    // 还在用自定义前缀。按产品定的规则：系统没了就回到安装引导，让人重新指定。
    if out
        .iter()
        .any(|item| item.available && item.source == "system")
    {
        if let Some(data_dir) = ctx.data_dir.as_ref() {
            clear_stored_connector_path(data_dir);
            cleanup_legacy_registration(data_dir, false);
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
/// macOS 没有随包分发可执行文件，沿用系统查找：标准安装位置 > 记住的路径 > PATH。
pub fn detect_all(ctx: &AppContext, custom_openconnect: &str) -> Vec<ConnectorInfo> {
    probe_app(AppId::Openconnect, ctx, custom_openconnect)
}

/// macOS：记住用户指定的 openconnect 位置，返回版本号与位置。
///
/// **只记路径**：不复制二进制、也不建软链。这里先后踩过几个坑：
/// - 复制：Homebrew 的 openconnect 通过绝对路径加载自己前缀里的动态库，拷出来的副本
///   只是碰巧能跑，`brew upgrade` 之后依赖失配；复制还会撞上自定义前缀的权限/隐私策略，
///   现场报的就是 `复制 openconnect 失败: Permission denied (os error 13)`；
/// - 复制/软链到数据目录：目录里出现可执行文件，会被用户当成「程序自带的副本」
///   （现场已误判成 Windows 内置文件），而且旧复制件在 brew 升级后过期还在。
///
/// 所以数据目录里只留一行路径（`<data>/vpn/openconnect-path.txt`），执行时直接用原文件；
/// 旧版本留下的副本/软链由 [`cleanup_legacy_registration`] 清掉。用户的选法不固定
/// （可执行文件 / bin 目录 / Cellar 版本目录），交给 `resolve_binary` 统一解析。
pub fn import_openconnect(data_dir: &Path, source: &Path) -> Result<ImportedOpenconnect, String> {
    let picked = apps::finder::resolve_binary(AppId::Openconnect, source)?;
    // Cellar 的版本目录会被 brew 升级换掉，能改指 prefix/bin 就改
    let binary = apps::finder::stable_binary_path(&picked);

    // 先探测：跑不起来就把原始输出带回去，别让用户猜
    let (text, ok) = crate::process::run_with_timeout(&binary, &["--version"], PROBE_TIMEOUT);
    let version = parse_openconnect_version(&text);
    if !ok || version.is_empty() {
        return Err(format!(
            "这份 openconnect {}",
            describe_launch_failure(&text, ok)
        ));
    }

    store_connector_path(data_dir, &binary)?;
    // 旧版本登记的副本/软链顺手清掉，数据目录里不再留可执行文件
    cleanup_legacy_registration(data_dir, false);
    Ok(ImportedOpenconnect {
        version,
        path: binary.to_string_lossy().to_string(),
    })
}

/// 删除记住的 openconnect 位置（连同旧版本留下的副本/软链）
pub fn remove_managed_openconnect(data_dir: &Path) -> Result<(), String> {
    clear_stored_connector_path(data_dir);
    cleanup_legacy_registration(data_dir, false);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::models::ConnectorKind;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ws-oc-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// 造一个能跑 `--version` 的假 openconnect（真跑一次，不依赖本机装没装）
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

    #[test]
    fn detect_all_returns_openconnect_channel() {
        // 空上下文下也要有结果（哪怕是 missing 占位），界面才能明确告诉用户「没找到」
        let ctx = AppContext::default();
        let found = detect_all(&ctx, "");
        assert!(found.iter().any(|item| item.kind == "openconnect"));
        // 只有一条通道，不该再冒出别的 kind
        assert!(found.iter().all(|item| item.kind == "openconnect"));
    }

    #[test]
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

    /// macOS 只向外探测：内置副本（Windows 的 exe）永远不能被当成候选。
    ///
    /// 现场后果：如果去探它，会得到一个「跑不起来（缺少依赖库或没有执行权限）」的
    /// 连接器条目，而不是干净的「没找到，请选择目录」—— 用户会以为程序在锁死一份
    /// 跑不了的 Windows 二进制。
    #[test]
    fn macos_never_probes_bundled_copy() {
        let location = AppLocation {
            id: AppId::Openconnect.as_str().to_string(),
            name: AppId::Openconnect.display_name().to_string(),
            description: AppId::Openconnect.description().to_string(),
            path: String::new(),
            available: false,
            source: "missing".to_string(),
            note: "没找到".to_string(),
            builtin_path: "/nonexistent/resources/apps/openconnect/openconnect.exe".to_string(),
            candidates: Vec::new(),
        };
        let ctx = AppContext::default();
        let found = probe_location(AppId::Openconnect, &ctx, location);
        assert!(
            found.iter().all(|item| item.source != "bundled"),
            "macOS 不能探测内置副本，实际: {:?}",
            found
                .iter()
                .map(|item| item.source.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            found.iter().any(|item| item.source == "missing"),
            "没找到时必须给出 missing 占位，实际: {:?}",
            found
                .iter()
                .map(|item| item.source.as_str())
                .collect::<Vec<_>>()
        );
    }

    /// 端到端：标准前缀里装了 openconnect 就必须识别成可用并带版本号。
    ///
    /// 这条用例正是「生产版识别不到」的回归护栏：从 Finder 启动的 .app 没有
    /// LANG/LC_ALL，openconnect 输出中文本地化版本行，旧解析拿不到版本号。
    /// 想复现当时的失败现场，用不带 locale 的环境直接跑测试二进制：
    ///   `env -i HOME="$HOME" <test-bin> --exact \
    ///    vpn::platform::macos::tests::standard_homebrew_openconnect_is_detected_without_locale_env`
    #[test]
    fn standard_homebrew_openconnect_is_detected_without_locale_env() {
        // 没装就跳过：用例不把「构建机装了 brew openconnect」当成前置条件
        let installed = [
            "/opt/homebrew/bin/openconnect",
            "/usr/local/bin/openconnect",
            "/opt/local/bin/openconnect",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file());
        if installed.is_none() {
            return;
        }

        let found = detect_all(&AppContext::default(), "");
        let picked = found.iter().find(|item| item.available).unwrap_or_else(|| {
            panic!("标准前缀里装好的 openconnect 必须被识别，实际: {:?}", found)
        });
        assert!(!picked.version.is_empty(), "必须解析出版本号: {:?}", picked);
        assert_eq!(picked.source, "system");
    }

    /// 探测子进程必须带 LC_ALL=C：否则中文系统上 `--version` 输出
    /// 「OpenConnect 版本 v9.21」，解析不到英文前缀就判成不可用。
    #[test]
    fn probe_runs_openconnect_under_c_locale() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("probe-locale");
        let dir = root.join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("openconnect");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho \"OpenConnect version ${LC_ALL:-unset}\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let info = probe_openconnect(&fake, "system");
        assert!(info.available, "note: {}", info.note);
        assert_eq!(info.version, "C", "探测必须把子进程 locale 钉成 C");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn import_rejects_directory_without_binary() {
        let source = std::env::temp_dir().join(format!("ws-oc-empty-{}", std::process::id()));
        let data = std::env::temp_dir().join(format!("ws-oc-data-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&source);
        let _ = std::fs::create_dir_all(&data);

        let error = import_openconnect(&data, &source).unwrap_err();
        assert!(error.contains("没有找到"), "实际: {}", error);

        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&data);
    }

    /// 核心行为：macOS 上「导入」只记住路径，不在数据目录里留副本或软链
    #[test]
    fn import_stores_path_without_copying_binary() {
        let root = temp_root("import-store-path");
        let source = fake_openconnect(&root.join("hb/bin"), "v9.99");
        let data = root.join("data");

        let imported = import_openconnect(&data, &source).unwrap();
        assert_eq!(imported.version, "v9.99");
        assert_eq!(
            imported.path,
            source.to_string_lossy().to_string(),
            "要回显记住的位置"
        );

        assert_eq!(
            read_stored_connector_path(&data).as_deref(),
            Some(source.as_path()),
            "数据目录里只应该记一行路径"
        );
        for dir in [
            apps::managed_dir(AppId::Openconnect, &data),
            data.join(apps::finder::APPS_DIR).join("openconnect"),
        ] {
            assert!(
                !dir.exists(),
                "数据目录里不能再出现副本/软链: {}",
                dir.display()
            );
        }
        assert!(source.is_file(), "源文件必须原样保留");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 用户截图里框的是 `Cellar/openconnect/9.21`，这一层也必须能导入，
    /// 并且记住的位置要是稳定的 `prefix/bin`（否则 brew 升级后路径失效）
    #[test]
    fn import_accepts_homebrew_version_dir() {
        let root = temp_root("import-cellar");
        let prefix = root.join("hb");
        let real = fake_openconnect(&prefix.join("Cellar/openconnect/9.21/bin"), "v9.21");
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::os::unix::fs::symlink(&real, prefix.join("bin/openconnect")).unwrap();
        let data = root.join("data");
        let stable = prefix.join("bin/openconnect");

        let imported = import_openconnect(&data, &prefix.join("Cellar/openconnect/9.21")).unwrap();
        assert_eq!(imported.version, "v9.21");
        assert_eq!(
            imported.path,
            stable.to_string_lossy().to_string(),
            "回显的位置要是稳定的 prefix/bin，不是带版本号的 Cellar 路径"
        );
        assert_eq!(
            read_stored_connector_path(&data).as_deref(),
            Some(stable.as_path()),
            "记住的也必须是稳定的 prefix/bin，不能钉死带版本号的 Cellar 路径"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 跑不起来的二进制要当场报错，并且不能留下半截托管副本
    #[test]
    fn import_reports_unrunnable_binary_and_leaves_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("import-broken");
        let dir = root.join("hb/bin");
        std::fs::create_dir_all(&dir).unwrap();
        let broken = dir.join("openconnect");
        std::fs::write(
            &broken,
            "#!/bin/sh\necho 'dyld: missing library' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
        let data = root.join("data");

        let error = import_openconnect(&data, &broken).unwrap_err();
        assert!(error.contains("跑不起来"), "实际: {}", error);
        assert!(
            read_stored_connector_path(&data).is_none(),
            "失败时不能留下登记"
        );
        assert!(
            !apps::managed_dir(AppId::Openconnect, &data).exists(),
            "失败时不能留下副本目录"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 生产版「选择安装目录」报错的原文：openconnect 明明能跑，只是 `--version` 输出
    /// 被系统语言翻译成中文（Finder 启动的 .app 没有 LANG）。旧解析认不出中文版本行，
    /// 界面就说成「跑不起来（缺少依赖库或没有执行权限）」—— 病根不在依赖，在版本行。
    #[test]
    fn import_accepts_localized_version_line() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("import-localized");
        let dir = root.join("hb/bin");
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("openconnect");
        // 写死中文输出：模拟中文系统上的真实 --version 文本，不依赖跑 locale
        std::fs::write(
            &fake,
            "#!/bin/sh\necho 'OpenConnect 版本 v9.21'\n\
             echo '正使用 GnuTLS 3.8.13。当前功能： PKCS#11, RSA software token'\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let data = root.join("data");

        let imported = import_openconnect(&data, &fake).expect("中文版本行也要能登记成功");
        assert_eq!(imported.version, "v9.21");
        assert_eq!(
            read_stored_connector_path(&data).as_deref(),
            Some(fake.as_path()),
            "中文版本行也要能记住路径"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 能执行但输出里没有版本行：必须报「认不出版本号」，不能说成「跑不起来」。
    /// 上一版就是这里误导排查方向的（现场原始输出其实是正常的中文 --version）。
    #[test]
    fn import_reports_unparseable_version_instead_of_missing_deps() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("import-unparseable");
        let dir = root.join("hb/bin");
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("openconnect");
        std::fs::write(&fake, "#!/bin/sh\necho 'some unexpected wrapper output'\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let data = root.join("data");

        let error = import_openconnect(&data, &fake).unwrap_err();
        assert!(
            error.contains("没从 --version 输出里识别出版本号"),
            "实际: {}",
            error
        );
        assert!(
            !error.contains("跑不起来"),
            "解析失败不能说成缺依赖/没权限: {}",
            error
        );
        assert!(
            read_stored_connector_path(&data).is_none(),
            "失败时不能留下登记"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 探测结果的备注同样要区分「跑不起来」和「能跑但认不出版本号」
    #[test]
    fn probe_note_separates_parse_failure_from_launch_failure() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("probe-note");
        let dir = root.join("hb/bin");
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("openconnect");
        std::fs::write(&fake, "#!/bin/sh\necho 'no version here'\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let info = probe_openconnect(&fake, "custom");
        assert!(!info.available);
        assert!(
            info.note.contains("没从 --version 输出里识别出版本号"),
            "实际: {}",
            info.note
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 端到端：先前「指定位置」登记过一份，后来 openconnect 换到了标准路径
    ///
    /// 点「检测」必须落到系统那份（`source == "system"`），登记的那份不再参与；
    /// 界面上「已指定 openconnect + 路径」那行就是靠这个结果消失的。
    #[test]
    fn detection_prefers_standard_install_over_registration() {
        // 标准安装位置：本机没有就跳过（CI 上大概率没装），有但要先确认它真能跑
        let standard = [
            "/opt/homebrew/bin/openconnect",
            "/usr/local/bin/openconnect",
            "/opt/local/bin/openconnect",
        ]
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
        // 模拟「指定位置」登记：数据目录里记了自定义前缀的路径
        let custom = fake_openconnect(&root.join("hb/bin"), "v0.0.1-custom");
        store_connector_path(&data, &custom).unwrap();

        let ctx = AppContext {
            data_dir: Some(data.clone()),
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
            "标准路径里有能跑的 openconnect 时，不再用「指定位置」记住的那份"
        );
        assert_eq!(available[0].path, standard.to_string_lossy().to_string());
        assert!(!available[0].version.is_empty(), "要带上探测到的版本号");
        assert!(
            read_stored_connector_path(&data).is_none(),
            "记住的路径要被删掉：系统没了就回安装引导重新指定，不能偷偷顶着用"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 旧版本把 openconnect 软链到 `<data>/apps/openconnect/`：清理时要把真实目标
    /// 转成路径记录，再删掉软链目录，用户不用重新选一次。
    #[test]
    fn legacy_symlink_is_converted_to_stored_path() {
        let root = temp_root("legacy-symlink");
        let data = root.join("data");
        let target = fake_openconnect(&root.join("hb/bin"), "v9.21");
        let legacy = data.join(apps::finder::APPS_DIR).join("openconnect");
        std::fs::create_dir_all(&legacy).unwrap();
        std::os::unix::fs::symlink(&target, legacy.join("openconnect")).unwrap();

        cleanup_legacy_registration(&data, true);

        assert_eq!(
            read_stored_connector_path(&data).as_deref(),
            Some(target.as_path()),
            "软链的真实目标要变成路径记录"
        );
        assert!(!legacy.exists(), "旧软链目录要删掉");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 旧版本复制出来的副本（普通文件）：必须删掉而不是继续用。
    ///
    /// 现场截图里的 `apps/openconnect/openconnect` 就是这种 97 KB 的复制件 ——
    /// brew 升级后会变成过期副本，数据目录里留着还会被误认为程序自带的文件。
    #[test]
    fn legacy_copied_binary_is_removed_not_reused() {
        let root = temp_root("legacy-copy");
        let data = root.join("data");
        let legacy = data.join(apps::finder::APPS_DIR).join("openconnect");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("openconnect"), b"old copied binary").unwrap();

        cleanup_legacy_registration(&data, true);

        assert!(!legacy.exists(), "旧复制件要删掉");
        assert!(
            read_stored_connector_path(&data).is_none(),
            "复制件没有真实来源，不能变成路径记录（需要用户重新选一次目录）"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 连接器偏好只认 openconnect：这一层与平台无关，但放在 mac 用例里做一次端到端确认
    #[test]
    fn detect_all_result_can_be_picked() {
        let ctx = AppContext::default();
        let found = detect_all(&ctx, "");
        // 没有可用连接器时 pick 就是 None，有可用时必须能选出来
        let picked = crate::vpn::platform::pick_connector(&found, ConnectorKind::Auto);
        assert_eq!(picked.is_some(), found.iter().any(|item| item.available));
    }
}
