use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tauri::{AppHandle, Emitter};
use ignore::WalkBuilder;
use crate::models::*;

/// 检查路径中是否有任何组件含空格
fn path_has_space(path: &Path) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s.contains(' ')
    })
}

/// 推送打包进度（无 AppHandle 时静默跳过，便于测试）
fn emit_progress(app: Option<&AppHandle>, progress: PackProgress) {
    if let Some(app) = app {
        let _ = app.emit("pack-progress", progress);
    }
}

/// 扫描源目录，返回需要打包的文件列表（已过滤排除规则 + 空格文件）
/// extra_excludes 中的路径若位于源目录内会被额外排除，避免把输出目录/zip 自身扫进来
fn scan_files(
    source: &Path,
    excludes: &[String],
    extra_excludes: &[&Path],
) -> Result<Vec<PathBuf>, String> {
    let mut all_excludes: Vec<String> = excludes.to_vec();

    for extra in extra_excludes {
        if !extra.starts_with(source) {
            continue;
        }
        if let Ok(rel) = extra.strip_prefix(source) {
            let rel = rel.to_string_lossy().to_string();
            // 空路径（即源目录自身）不能作为排除规则
            if !rel.is_empty() {
                all_excludes.push(rel);
            }
        }
    }

    let mut builder = WalkBuilder::new(source);
    builder.standard_filters(false);
    builder.follow_links(false);

    let mut override_builder = ignore::overrides::OverrideBuilder::new(source);
    for pattern in &all_excludes {
        override_builder.add(&format!("!{}", pattern))
            .map_err(|e| format!("排除规则 '{}' 解析失败: {}", pattern, e))?;
    }
    let overrides = override_builder.build()
        .map_err(|e| format!("排除规则构建失败: {}", e))?;
    builder.overrides(overrides);

    let walker = builder.build();
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in walker {
        let entry = entry.map_err(|e| format!("遍历错误: {}", e))?;
        if entry.file_type().map_or(false, |ft| ft.is_file()) {
            // 只检查相对源目录的路径：否则源目录本身含空格时会把所有文件都过滤掉
            let rel = entry.path().strip_prefix(source).unwrap_or(entry.path());
            if path_has_space(rel) {
                continue;
            }
            files.push(entry.into_path());
        }
    }
    Ok(files)
}

/// 把扫描结果复制到 dest_root，保持相对目录结构，并推送复制进度
/// 返回 (已复制数, 跳过数, 错误列表)
fn copy_files(
    app: Option<&AppHandle>,
    files: &[PathBuf],
    source: &Path,
    dest_root: &Path,
) -> (u64, u64, Vec<String>) {
    let mut copied: u64 = 0;
    let mut skipped: u64 = 0;
    let mut errors: Vec<String> = Vec::new();

    if files.is_empty() {
        return (copied, skipped, errors);
    }

    let total = files.len() as u64;

    for (i, file_path) in files.iter().enumerate() {
        // 必须用遍历时的 source 去匹配：canonicalize 会把 /var 解析成 /private/var，
        // 与遍历出的路径不一致，一旦退化成绝对路径，dest_root.join() 会直接丢弃基路径
        let rel = match file_path.strip_prefix(source) {
            Ok(rel) => rel,
            Err(_) => {
                errors.push(format!("路径不在源目录内，已跳过: {}", file_path.display()));
                skipped += 1;
                continue;
            }
        };
        let dest = dest_root.join(rel);

        if let Some(parent) = dest.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                errors.push(format!("创建目录失败 {}: {}", parent.display(), e));
                skipped += 1;
                continue;
            }
        }

        match fs::copy(file_path, &dest) {
            Ok(_) => {
                copied += 1;
                if (i + 1) % 50 == 0 || i + 1 == files.len() {
                    emit_progress(app, PackProgress {
                        phase: "copying".to_string(),
                        current: (i + 1) as u64,
                        total,
                        current_file: rel.to_string_lossy().to_string(),
                        percentage: ((i + 1) as f64 / total as f64) * 100.0,
                    });
                }
            }
            Err(e) => {
                errors.push(format!("复制失败 {}: {}", file_path.display(), e));
                skipped += 1;
            }
        }
    }

    (copied, skipped, errors)
}

/// 打包项目：提取文件到输出目录（执行"开始打包"）
pub fn pack_project(
    app: &AppHandle,
    project: &ProjectConfig,
    default_exclude: &[String],
) -> Result<PackResult, String> {
    pack_project_inner(Some(app), project, default_exclude)
}

/// "开始打包"的核心逻辑：遍历源目录 → 过滤排除规则 → 清空输出目录 → 复制文件
fn pack_project_inner(
    app: Option<&AppHandle>,
    project: &ProjectConfig,
    default_exclude: &[String],
) -> Result<PackResult, String> {
    let start = Instant::now();
    let source = Path::new(&project.source_dir);

    if !source.exists() {
        return Err(format!("源目录不存在: {}", project.source_dir));
    }

    let output_path = Path::new(&project.output_dir);

    // 合并排除规则
    let mut all_excludes: Vec<String> = default_exclude.to_vec();
    all_excludes.extend(project.exclude.iter().cloned());

    // 阶段1：扫描文件
    emit_progress(app, PackProgress {
        phase: "scanning".to_string(),
        current: 0,
        total: 0,
        current_file: String::new(),
        percentage: 0.0,
    });

    let files = scan_files(source, &all_excludes, &[output_path])?;
    let total = files.len() as u64;

    if total == 0 {
        emit_progress(app, PackProgress {
            phase: "done".to_string(),
            current: 0,
            total: 0,
            current_file: String::new(),
            percentage: 100.0,
        });
        return Ok(PackResult {
            success: true,
            total_files: 0,
            copied_files: 0,
            skipped_files: 0,
            elapsed_ms: start.elapsed().as_millis() as u64,
            errors: vec!["未找到可复制的文件".to_string()],
        });
    }

    // 阶段2：清空输出目录
    emit_progress(app, PackProgress {
        phase: "cleaning".to_string(),
        current: 0,
        total,
        current_file: String::new(),
        percentage: 0.0,
    });

    if output_path.exists() {
        fs::remove_dir_all(output_path)
            .map_err(|e| format!("清空输出目录失败: {}", e))?;
    }
    fs::create_dir_all(output_path)
        .map_err(|e| format!("创建输出目录失败: {}", e))?;

    // 阶段3：复制文件
    let (copied, skipped, errors) = copy_files(app, &files, source, output_path);

    let result = PackResult {
        success: errors.is_empty(),
        total_files: total,
        copied_files: copied,
        skipped_files: skipped,
        elapsed_ms: start.elapsed().as_millis() as u64,
        errors,
    };

    emit_progress(app, PackProgress {
        phase: "done".to_string(),
        current: total,
        total,
        current_file: String::new(),
        percentage: 100.0,
    });

    Ok(result)
}

/// 打包项目为 ZIP
pub fn pack_to_zip(
    app: &AppHandle,
    project: &ProjectConfig,
    default_exclude: &[String],
) -> Result<PackResult, String> {
    pack_to_zip_inner(Some(app), project, default_exclude)
}

/// ZIP 打包核心逻辑：扫描过滤后把源文件直接流式写入 zip，无任何中间产物
/// zip 内每个条目包裹一层输出目录名
fn pack_to_zip_inner(
    app: Option<&AppHandle>,
    project: &ProjectConfig,
    default_exclude: &[String],
) -> Result<PackResult, String> {
    let start = Instant::now();
    let source = Path::new(&project.source_dir);

    if !source.exists() {
        return Err(format!("源目录不存在: {}", project.source_dir));
    }

    // zip 输出路径：output_dir 同级的 output_dir 文件名.zip
    let output_path = Path::new(&project.output_dir);
    let zip_path = output_path.with_extension("zip");
    // zip 内包裹的顶层目录名：取输出目录的文件夹名
    let wrapper = output_path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project.name.clone());

    // 合并排除规则
    let mut all_excludes: Vec<String> = default_exclude.to_vec();
    all_excludes.extend(project.exclude.iter().cloned());

    // 阶段1：扫描文件（顺带排除输出目录与 zip 自身，防止被扫进来）
    emit_progress(app, PackProgress {
        phase: "scanning".to_string(),
        current: 0,
        total: 0,
        current_file: String::new(),
        percentage: 0.0,
    });

    let files = scan_files(source, &all_excludes, &[output_path, &zip_path])?;
    let total = files.len() as u64;

    if total == 0 {
        emit_progress(app, PackProgress {
            phase: "done".to_string(),
            current: 0,
            total: 0,
            current_file: String::new(),
            percentage: 100.0,
        });
        return Ok(PackResult {
            success: true,
            total_files: 0,
            copied_files: 0,
            skipped_files: 0,
            elapsed_ms: start.elapsed().as_millis() as u64,
            errors: vec!["未找到可打包的文件".to_string()],
        });
    }

    // 阶段2：准备 zip 文件（清掉可能存在的旧同名文件/目录）
    if let Some(parent) = zip_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("创建 zip 所在目录失败: {}", e))?;
    }
    if zip_path.exists() {
        let metadata = fs::metadata(&zip_path)
            .map_err(|e| format!("读取 zip 路径信息失败: {}", e))?;
        if metadata.is_dir() {
            fs::remove_dir_all(&zip_path)
                .map_err(|e| format!("删除旧的同名文件夹失败: {}", e))?;
        } else {
            fs::remove_file(&zip_path)
                .map_err(|e| format!("删除旧的 zip 文件失败: {}", e))?;
        }
    }

    let zip_file = fs::File::create(&zip_path)
        .map_err(|e| format!("创建 zip 文件失败: {}", e))?;
    let mut zip = zip::ZipWriter::new(zip_file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut zipped: u64 = 0;
    let mut skipped: u64 = 0;
    let mut errors: Vec<String> = Vec::new();

    // 阶段3：边读源文件边写入 zip（不落地任何中间产物）
    for (i, file_path) in files.iter().enumerate() {
        let rel = match file_path.strip_prefix(source) {
            Ok(rel) => rel,
            Err(_) => {
                errors.push(format!("路径不在源目录内，已跳过: {}", file_path.display()));
                skipped += 1;
                continue;
            }
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        // zip 内包裹一层目录名
        let entry_name = format!("{}/{}", wrapper, rel_str);

        match fs::File::open(file_path) {
            Ok(mut f) => {
                if let Err(e) = zip.start_file(&entry_name, options) {
                    errors.push(format!("写入 zip 失败 {}: {}", entry_name, e));
                    skipped += 1;
                    continue;
                }
                // 流式复制，不一次性读入内存
                if let Err(e) = std::io::copy(&mut f, &mut zip) {
                    errors.push(format!("写入 zip 失败 {}: {}", entry_name, e));
                    skipped += 1;
                    continue;
                }
                zipped += 1;
                if (i + 1) % 50 == 0 || i + 1 == files.len() {
                    emit_progress(app, PackProgress {
                        phase: "zipping".to_string(),
                        current: (i + 1) as u64,
                        total,
                        current_file: rel_str,
                        percentage: ((i + 1) as f64 / total as f64) * 100.0,
                    });
                }
            }
            Err(e) => {
                errors.push(format!("打开文件失败 {}: {}", file_path.display(), e));
                skipped += 1;
            }
        }
    }

    if let Err(e) = zip.finish() {
        errors.push(format!("完成 zip 文件失败: {}", e));
    }

    emit_progress(app, PackProgress {
        phase: "done".to_string(),
        current: total,
        total,
        current_file: zip_path.to_string_lossy().to_string(),
        percentage: 100.0,
    });

    Ok(PackResult {
        success: errors.is_empty(),
        total_files: total,
        copied_files: zipped,
        skipped_files: skipped,
        elapsed_ms: start.elapsed().as_millis() as u64,
        errors,
    })
}

/// 验证项目配置
pub fn validate_project(project: &ProjectConfig) -> ProjectValidation {
    let source = Path::new(&project.source_dir);
    let mut warnings: Vec<String> = Vec::new();

    if !source.exists() {
        return ProjectValidation {
            valid: false,
            source_exists: false,
            file_count: 0,
            warnings: vec![format!("源目录不存在: {}", project.source_dir)],
        };
    }

    let file_count = count_files(source);

    if project.output_dir.is_empty() {
        warnings.push("输出目录未设置".to_string());
    }

    let output = Path::new(&project.output_dir);
    if output.exists() && output == source {
        warnings.push("输出目录与源目录相同".to_string());
    }

    ProjectValidation {
        valid: source.exists() && !project.output_dir.is_empty(),
        source_exists: source.exists(),
        file_count,
        warnings,
    }
}

fn count_files(path: &Path) -> u64 {
    let mut count = 0u64;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_file() {
                    count += 1;
                } else if ft.is_dir() {
                    count += count_files(&entry.path());
                }
            }
        }
    }
    count
}

/// 检测项目类型
pub fn detect_project_type(source_dir: &str) -> ProjectType {
    let source = Path::new(source_dir);
    if !source.exists() {
        return ProjectType::Unknown;
    }

    // 检测 Vue 项目（仅使用 Vue 特有的强特征，避免误判 Layui 项目）
    let pkg_path = source.join("package.json");
    if pkg_path.exists() {
        if let Ok(content) = fs::read_to_string(&pkg_path) {
            if content.contains("\"vue\"") {
                return ProjectType::Vue;
            }
        }
    }
    if source.join("vite.config.js").exists()
        || source.join("vite.config.ts").exists()
        || source.join("vite.config.mjs").exists()
        || source.join("vue.config.js").exists()
        || source.join("vue.config.ts").exists()
        || source.join("src/App.vue").exists()
    {
        return ProjectType::Vue;
    }

    // 检测 Layui 项目
    // 查找 layui 相关文件，最多往下找 3 层
    fn find_layui(path: &Path, depth: usize) -> bool {
        if depth > 3 {
            return false;
        }
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(ft) = entry.file_type() {
                    if ft.is_file() {
                        let name = path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_lowercase();
                        if (name.starts_with("layui") && name.ends_with(".js"))
                            || (name.starts_with("layui") && name.ends_with(".css"))
                            || name == "layui.all.js"
                        {
                            return true;
                        }
                    } else if ft.is_dir() {
                        let dir_name = path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_lowercase();
                        // 跳过 node_modules
                        if dir_name == "node_modules" {
                            continue;
                        }
                        if find_layui(&path, depth + 1) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    if find_layui(source, 0) {
        return ProjectType::Layui;
    }

    ProjectType::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn make_project(source: &Path, output_dir: &Path) -> ProjectConfig {
        ProjectConfig {
            id: "test-id".to_string(),
            name: "测试项目".to_string(),
            source_dir: source.to_string_lossy().to_string(),
            output_dir: output_dir.to_string_lossy().to_string(),
            exclude: Vec::new(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn zip_entries(zip_path: &Path) -> Vec<String> {
        let file = fs::File::open(zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect()
    }

    fn zip_read_to_string(zip_path: &Path, name: &str) -> String {
        use std::io::Read;
        let file = fs::File::open(zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name(name).unwrap();
        let mut s = String::new();
        entry.read_to_string(&mut s).unwrap();
        s
    }

    /// ZIP 打包直接流式写 zip：不产生中间产物、zip 内包裹一层目录名、含空格文件被过滤
    #[test]
    fn zip_streams_directly_with_dir_wrapper() {
        let src = tempfile::tempdir().unwrap();
        write_file(src.path(), "index.html", "<html></html>");
        write_file(src.path(), "css/app.css", "body{}");
        write_file(src.path(), "js/app.js", "var a=1;");
        write_file(src.path(), "有 空格.js", "x");

        let out_parent = tempfile::tempdir().unwrap();
        let output_dir = out_parent.path().join("layui-dist");
        let project = make_project(src.path(), &output_dir);

        let result = pack_to_zip_inner(None, &project, &[]).unwrap();
        assert!(result.success, "打包失败: {:?}", result.errors);

        // 1) 无中间产物：输出目录不应被创建
        assert!(!output_dir.exists(), "输出目录被创建了，说明还在落中间产物");

        // 2) zip 生成在输出目录同级
        let zip_path = output_dir.with_extension("zip");
        assert!(zip_path.exists(), "zip 未生成: {}", zip_path.display());

        // 3) zip 内包裹一层目录名
        let entries = zip_entries(&zip_path);
        for expect in [
            "layui-dist/index.html",
            "layui-dist/css/app.css",
            "layui-dist/js/app.js",
        ] {
            assert!(
                entries.contains(&expect.to_string()),
                "缺少条目 {}，实际: {:?}",
                expect,
                entries
            );
        }

        // 4) 内容确实被完整写入（防止写出 0 字节文件）
        assert_eq!(
            zip_read_to_string(&zip_path, "layui-dist/index.html"),
            "<html></html>"
        );
        assert_eq!(
            zip_read_to_string(&zip_path, "layui-dist/css/app.css"),
            "body{}"
        );

        // 5) 含空格的文件被过滤
        assert!(
            entries.iter().all(|e| !e.contains(' ')),
            "含空格文件未被过滤: {:?}",
            entries
        );

        assert_eq!(result.total_files, 3);
        assert_eq!(result.copied_files, 3);
        assert_eq!(result.skipped_files, 0);
    }

    /// 源目录自身含空格时，不应把所有文件都过滤掉
    #[test]
    fn source_dir_with_space_keeps_files() {
        let parent = tempfile::tempdir().unwrap();
        let src = parent.path().join("my project");
        fs::create_dir_all(&src).unwrap();
        write_file(&src, "index.html", "x");
        write_file(&src, "js/app.js", "y");

        let out_parent = tempfile::tempdir().unwrap();
        let output_dir = out_parent.path().join("dist");
        let project = make_project(&src, &output_dir);

        let result = pack_to_zip_inner(None, &project, &[]).unwrap();
        assert!(result.success, "{:?}", result.errors);
        assert_eq!(result.total_files, 2, "源目录含空格时不应过滤文件");
    }

    /// "开始打包"仍然落地到输出目录
    #[test]
    fn pack_project_extracts_to_output_dir() {
        let src = tempfile::tempdir().unwrap();
        write_file(src.path(), "a.html", "a");
        write_file(src.path(), "sub/b.js", "b");

        let out_parent = tempfile::tempdir().unwrap();
        let output_dir = out_parent.path().join("dist");
        let project = make_project(src.path(), &output_dir);

        let result = pack_project_inner(None, &project, &[]).unwrap();
        assert!(result.success, "{:?}", result.errors);
        assert!(output_dir.join("a.html").exists());
        assert!(output_dir.join("sub/b.js").exists());
        assert_eq!(result.copied_files, 2);
    }

    /// 输出目录位于源目录内部时，不应把它自己（含旧内容）扫进包里
    #[test]
    fn nested_output_dir_is_excluded() {
        let src = tempfile::tempdir().unwrap();
        write_file(src.path(), "a.html", "a");

        let output_dir = src.path().join("dist");
        fs::create_dir_all(&output_dir).unwrap();
        write_file(&output_dir, "stale.html", "old");

        let project = make_project(src.path(), &output_dir);
        let result = pack_to_zip_inner(None, &project, &[]).unwrap();
        assert!(result.success, "{:?}", result.errors);

        let entries = zip_entries(&output_dir.with_extension("zip"));
        assert!(
            entries.contains(&"dist/a.html".to_string()),
            "条目: {:?}",
            entries
        );
        assert!(
            !entries.iter().any(|e| e.contains("stale")),
            "旧输出目录内容被扫进来了: {:?}",
            entries
        );
    }
}
