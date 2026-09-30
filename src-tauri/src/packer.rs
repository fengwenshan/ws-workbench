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

/// zip 头部的 UTF-8 标志位（general purpose bit 11）
#[cfg(test)]
const ZIP_UTF8_FLAG: u16 = 0x0800;

#[cfg(test)]
fn read_u16(bytes: &[u8], at: usize) -> usize {
    u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize
}

// 条目名一律写成 UTF-8 字节，并保留 zip crate 自动置上的 UTF-8 标志位(bit 11)。
// 这是 ZIP 规格推荐做法：标准 Info-ZIP unzip 只有看到该标志位才知道名字是 UTF-8，
// 缺标志位时它会按 CP437 转码，中文就成乱码（macOS 自带 unzip 是 Apple 定制版，
// 对缺标志位的名字原样落盘，所以在 macOS 上复现不出来）。

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
    // 条目名一律写成 UTF-8 字节；zip crate 会给非 ASCII 名置 UTF-8 标志位(bit 11)，
    // 该标志位保持置位（原因见文件上方注释）
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

/// 从 package.json 里取出某个依赖的版本号（粗略解析，够用即可）
fn dep_version(content: &str, name: &str) -> Option<String> {
    let key = format!("\"{}\"", name);
    let idx = content.find(&key)?;
    let rest = content[idx + key.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 检测前端项目：Vue2 / Vue3 / React
/// 版本优先看 package.json 的依赖声明，没有声明时用构建配置与入口文件兜底
fn detect_frontend(source: &Path) -> Option<ProjectType> {
    if let Ok(content) = fs::read_to_string(source.join("package.json")) {
        if let Some(version) = dep_version(&content, "vue") {
            // 主版本号是 3 → Vue3，其余（含 ^2 / ~2 / workspace:* 之类）按 Vue2
            let major_is_3 = version
                .trim_start_matches(|c: char| !c.is_ascii_digit())
                .starts_with('3');
            return Some(if major_is_3 {
                ProjectType::Vue3
            } else {
                ProjectType::Vue2
            });
        }
        if dep_version(&content, "react").is_some() {
            return Some(ProjectType::React);
        }
    }

    // 没有依赖声明时的兜底：Vue CLI 的 vue.config.* 按 Vue2，其余有 .vue 入口的按 Vue3
    if source.join("vue.config.js").exists() || source.join("vue.config.ts").exists() {
        return Some(ProjectType::Vue2);
    }
    if source.join("src/App.vue").exists() {
        return Some(ProjectType::Vue3);
    }
    if ["src/App.jsx", "src/App.tsx", "src/main.jsx", "src/main.tsx"]
        .iter()
        .any(|f| source.join(f).exists())
    {
        return Some(ProjectType::React);
    }

    None
}

/// 找出 Java 项目的构建文件内容（pom.xml / build.gradle），供判断框架用
fn read_java_build_file(dir: &Path) -> Option<String> {
    for name in ["pom.xml", "build.gradle", "build.gradle.kts"] {
        let path = dir.join(name);
        if path.is_file() {
            if let Ok(content) = fs::read_to_string(&path) {
                return Some(content.to_lowercase());
            }
        }
    }
    None
}

/// Java 项目的特征文件（没有构建文件的老项目靠这些认出来）
#[derive(Default)]
struct JavaMarkers {
    java: bool,
    spring: bool,
    struts: bool,
}

/// 浅层扫描（最多 3 层）Java 项目的特征文件
fn scan_java_markers(path: &Path, depth: usize, markers: &mut JavaMarkers) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if matches!(
                name.as_str(),
                "node_modules" | "target" | "out" | ".git" | ".svn" | ".idea"
            ) {
                continue;
            }
            if name == "web-inf" || (name == "java" && child.ends_with(Path::new("src/main/java"))) {
                markers.java = true;
            }
            scan_java_markers(&child, depth + 1, markers);
        } else {
            match name.as_str() {
                "struts.xml" => markers.struts = true,
                "web.xml" | ".classpath" | "pom.xml" => markers.java = true,
                _ => {
                    if name.ends_with(".xml")
                        && (name.contains("context") || name.contains("dispatcher"))
                    {
                        markers.spring = true;
                    }
                }
            }
        }
    }
}

/// 检测 Java 项目：Spring Boot / Struts2 / Spring / 纯 Java
///
/// 源目录也可能只是项目里的一个子目录（如 src/main/webapp），所以构建文件允许往上找 3 层。
fn detect_java(source: &Path) -> Option<ProjectType> {
    let mut cursor: Option<&Path> = Some(source);
    let mut depth = 0;
    while let Some(dir) = cursor {
        if let Some(build) = read_java_build_file(dir) {
            if build.contains("spring-boot") {
                return Some(ProjectType::SpringBoot);
            }
            if build.contains("struts") {
                return Some(ProjectType::Struts2);
            }
            if build.contains("spring") {
                return Some(ProjectType::Spring);
            }
            return Some(ProjectType::Java);
        }
        if depth >= 3 {
            break;
        }
        cursor = dir.parent();
        depth += 1;
    }

    let mut markers = JavaMarkers::default();
    scan_java_markers(source, 0, &mut markers);
    if markers.struts {
        return Some(ProjectType::Struts2);
    }
    if markers.spring {
        return Some(ProjectType::Spring);
    }
    if markers.java {
        return Some(ProjectType::Java);
    }

    None
}

/// 查找 layui 相关文件，最多往下找 3 层
fn find_layui(path: &Path, depth: usize) -> bool {
    if depth > 3 {
        return false;
    }
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(ft) = entry.file_type() {
                if ft.is_file() {
                    let name = path
                        .file_name()
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
                    let dir_name = path
                        .file_name()
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

/// 检测项目类型
pub fn detect_project_type(source_dir: &str) -> ProjectType {
    let source = Path::new(source_dir);
    if !source.exists() {
        return ProjectType::Unknown;
    }

    if let Some(kind) = detect_frontend(source) {
        return kind;
    }

    if find_layui(source, 0) {
        return ProjectType::Layui;
    }

    if let Some(kind) = detect_java(source) {
        return kind;
    }

    ProjectType::Unknown
}

// ===== 版本控制信息检测 =====

/// 从起始目录逐级向上查找版本控制根目录
/// .git / .svn 可能位于源目录的上级（例如源目录只是仓库的一个子目录）
fn find_vcs_root(start: &Path) -> Option<(VcsType, PathBuf)> {
    let mut cursor: Option<&Path> = Some(start);
    while let Some(dir) = cursor {
        if dir.join(".git").exists() {
            return Some((VcsType::Git, dir.to_path_buf()));
        }
        if dir.join(".svn").is_dir() {
            return Some((VcsType::Svn, dir.to_path_buf()));
        }
        cursor = dir.parent().filter(|p| !p.as_os_str().is_empty());
    }
    None
}

/// 解析 git config 中的远程地址：优先 origin，否则取第一个 url
fn parse_git_config_url(content: &str) -> Option<String> {
    let mut origin: Option<String> = None;
    let mut first: Option<String> = None;
    let mut in_origin = false;

    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            // 只关心 [remote "origin"] 段，其余段内的 url 一律忽略
            in_origin = line.replace(' ', "") == "[remote\"origin\"]";
            continue;
        }
        let Some(rest) = line.strip_prefix("url") else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim().to_string();
        if value.is_empty() {
            continue;
        }
        if first.is_none() {
            first = Some(value.clone());
        }
        if in_origin {
            origin = Some(value);
        }
    }

    origin.or(first)
}

/// 定位 git 配置文件：普通仓库为 <root>/.git/config，worktree/submodule 下 .git 是文件
fn git_config_path(root: &Path) -> Option<PathBuf> {
    let git_path = root.join(".git");
    if git_path.is_dir() {
        return Some(git_path.join("config"));
    }
    let content = fs::read_to_string(&git_path).ok()?;
    let git_dir = content.trim().strip_prefix("gitdir:")?.trim();
    if git_dir.is_empty() {
        return None;
    }
    let path = PathBuf::from(git_dir);
    let abs = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    Some(abs.join("config"))
}

const URL_SCHEMES: [&str; 5] = ["svn+ssh://", "svn://", "https://", "http://", "ssh://"];

fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '-' | '.' | '_' | '~' | ':' | '/' | '?' | '#' | '[' | ']' | '@' | '!' | '$' | '&'
                | '\'' | '(' | ')' | '*' | '+' | ',' | ';' | '=' | '%'
        )
}

/// 去掉 URL 末尾可能粘连的标点（wc.db 中 URL 与后续字段无分隔符时会出现）
fn trim_url_tail(url: &str) -> &str {
    url.trim_end_matches(|c| matches!(c, '.' | ',' | ';' | '\'' | '"' | ')' | ']' | '}'))
}

/// 从二进制文本中提取仓库地址：取出现次数最多的一条，次数相同取最先出现的
fn extract_repo_url(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    // (地址, 首次出现位置, 出现次数)
    let mut candidates: Vec<(String, usize, usize)> = Vec::new();

    for scheme in URL_SCHEMES {
        let mut from = 0usize;
        while let Some(offset) = text[from..].find(scheme) {
            let start = from + offset;
            let end = text[start..]
                .char_indices()
                .find(|(_, c)| !is_url_char(*c))
                .map(|(i, _)| start + i)
                .unwrap_or(text.len());
            let url = trim_url_tail(&text[start..end]);
            if url.len() > scheme.len() {
                match candidates.iter_mut().find(|(u, _, _)| u == url) {
                    Some(entry) => entry.2 += 1,
                    None => candidates.push((url.to_string(), start, 1)),
                }
            }
            from = if end > start { end } else { start + 1 };
        }
    }

    candidates
        .into_iter()
        .max_by(|a, b| a.2.cmp(&b.2).then(b.1.cmp(&a.1)))
        .map(|(url, _, _)| url)
}

/// 通过 svn 命令行读取工作副本地址
fn svn_cli_url(root: &Path) -> Option<String> {
    let output = std::process::Command::new("svn")
        .arg("info")
        .arg("--show-item")
        .arg("url")
        .arg(root)
        .output()
        .ok()?;
    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !url.is_empty() {
            return Some(url);
        }
    }

    // 兼容不支持 --show-item 的旧版本 svn，退回到解析 info 输出的 URL 行
    let output = std::process::Command::new("svn")
        .arg("info")
        .arg(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).lines().find_map(|line| {
        line.trim()
            .strip_prefix("URL:")
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
    })
}

/// 获取 svn 工作副本地址：优先命令行，未安装 svn 时退回到工作副本数据库
fn svn_remote_url(root: &Path) -> Option<String> {
    if let Some(url) = svn_cli_url(root) {
        return Some(url);
    }
    fs::read(root.join(".svn").join("wc.db"))
        .ok()
        .and_then(|bytes| extract_repo_url(&bytes))
}

/// 检测源目录的版本控制类型与远程仓库地址
pub fn detect_vcs(source_dir: &str) -> VcsInfo {
    let none = VcsInfo {
        vcs_type: VcsType::None,
        url: String::new(),
    };

    let source = Path::new(source_dir);
    if !source.exists() {
        return none;
    }
    // 统一成绝对路径，向上查找才不会被相对路径的 parent() 截断
    let start = match fs::canonicalize(source) {
        Ok(path) => path,
        Err(_) => return none,
    };

    let Some((vcs_type, root)) = find_vcs_root(&start) else {
        return none;
    };

    let url = match vcs_type {
        VcsType::Git => git_config_path(&root)
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|content| parse_git_config_url(&content)),
        VcsType::Svn => svn_remote_url(&root),
        VcsType::None => None,
    };

    VcsInfo {
        vcs_type,
        url: url.unwrap_or_default(),
    }
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

    /// 条目名必须是 UTF-8 字节，并置 UTF-8 标志位(bit 11)。
    /// 标准 Info-ZIP unzip 靠该标志位识别 UTF-8 名字；缺标志位时它按 CP437 转码，中文会乱码。
    #[test]
    fn zip_entry_names_are_utf8_flagged() {
        let src = tempfile::tempdir().unwrap();
        write_file(src.path(), "中文目录/中文文件.js", "var a=1;");

        let out_parent = tempfile::tempdir().unwrap();
        let output_dir = out_parent.path().join("ws-dist");
        let project = make_project(src.path(), &output_dir);

        let result = pack_to_zip_inner(None, &project, &[]).unwrap();
        assert!(result.success, "打包失败: {:?}", result.errors);

        let bytes = fs::read(output_dir.with_extension("zip")).unwrap();
        assert_eq!(&bytes[0..4], b"PK\x03\x04", "不是 zip 本地文件头");

        // 名字仍是 UTF-8 字节
        let name_len = read_u16(&bytes, 26);
        let name = std::str::from_utf8(&bytes[30..30 + name_len]).unwrap();
        assert_eq!(name, "ws-dist/中文目录/中文文件.js");

        // 本地文件头与中央目录都要置 UTF-8 标志位
        assert_ne!(
            u16::from_le_bytes([bytes[6], bytes[7]]) & ZIP_UTF8_FLAG,
            0,
            "本地头应置 UTF-8 标志(bit 11)"
        );
        let c = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
        assert_ne!(
            u16::from_le_bytes([bytes[c + 8], bytes[c + 9]]) & ZIP_UTF8_FLAG,
            0,
            "中央目录应置 UTF-8 标志(bit 11)"
        );
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

    /// 默认规则中的 ".*" 系列：以 . 开头的文件/文件夹（含子目录里的）全部被排除
    #[test]
    fn dot_entries_are_excluded_by_default_rules() {
        let src = tempfile::tempdir().unwrap();
        write_file(src.path(), "index.html", "<html></html>");
        write_file(src.path(), "js/app.js", "var a=1;");
        write_file(src.path(), ".env", "SECRET=1");
        write_file(src.path(), ".git/config", "[core]");
        write_file(src.path(), ".vscode/settings.json", "{}");
        write_file(src.path(), "js/.cache/tmp.js", "x");
        write_file(src.path(), "js/lib/.hidden.js", "y");

        let out_parent = tempfile::tempdir().unwrap();
        let output_dir = out_parent.path().join("dist");
        let project = make_project(src.path(), &output_dir);

        let excludes = crate::models::default_exclude_rules();
        let result = pack_to_zip_inner(None, &project, &excludes).unwrap();
        assert!(result.success, "{:?}", result.errors);

        let entries = zip_entries(&output_dir.with_extension("zip"));
        for expect in ["dist/index.html", "dist/js/app.js"] {
            assert!(
                entries.contains(&expect.to_string()),
                "缺少条目 {}，实际: {:?}",
                expect,
                entries
            );
        }
        for entry in &entries {
            let rel = entry.strip_prefix("dist/").unwrap_or(entry);
            assert!(
                rel.split('/').all(|c| !c.starts_with('.')),
                "含 . 开头的路径未被排除: {:?}",
                entries
            );
        }
        assert_eq!(result.total_files, 2, "条目: {:?}", entries);
    }

    fn type_of(dir: &Path) -> ProjectType {
        detect_project_type(&dir.to_string_lossy())
    }

    /// 前端：按 package.json 里的 vue 主版本区分 Vue2 / Vue3，react 识别为 React
    #[test]
    fn detects_frontend_projects() {
        let vue3 = tempfile::tempdir().unwrap();
        write_file(vue3.path(), "package.json", "{ \"dependencies\": { \"vue\": \"^3.4.0\" } }");
        assert_eq!(type_of(vue3.path()), ProjectType::Vue3);

        let vue2 = tempfile::tempdir().unwrap();
        write_file(vue2.path(), "package.json", "{ \"dependencies\": { \"vue\": \"^2.6.14\" } }");
        assert_eq!(type_of(vue2.path()), ProjectType::Vue2);

        let react = tempfile::tempdir().unwrap();
        write_file(react.path(), "package.json", "{ \"dependencies\": { \"react\": \"^18.2.0\" } }");
        assert_eq!(type_of(react.path()), ProjectType::React);
    }

    /// 前端兜底：没有 package.json 时靠 vue.config.* / .vue / jsx 入口判断
    #[test]
    fn detects_frontend_without_package_json() {
        let cli = tempfile::tempdir().unwrap();
        write_file(cli.path(), "vue.config.js", "module.exports = {}");
        write_file(cli.path(), "src/App.vue", "<template></template>");
        assert_eq!(type_of(cli.path()), ProjectType::Vue2);

        let vite = tempfile::tempdir().unwrap();
        write_file(vite.path(), "vite.config.ts", "export default {}");
        write_file(vite.path(), "src/App.vue", "<template></template>");
        assert_eq!(type_of(vite.path()), ProjectType::Vue3);

        let react = tempfile::tempdir().unwrap();
        write_file(react.path(), "src/App.jsx", "export default () => null");
        assert_eq!(type_of(react.path()), ProjectType::React);
    }

    /// Java：按构建文件里的依赖区分 Spring Boot / Struts2 / Spring / 纯 Java
    #[test]
    fn detects_java_projects_by_build_file() {
        let boot = tempfile::tempdir().unwrap();
        write_file(boot.path(), "pom.xml", "<artifactId>spring-boot-starter-web</artifactId>");
        assert_eq!(type_of(boot.path()), ProjectType::SpringBoot);

        let struts = tempfile::tempdir().unwrap();
        write_file(struts.path(), "pom.xml", "<artifactId>struts2-core</artifactId>");
        assert_eq!(type_of(struts.path()), ProjectType::Struts2);

        let spring = tempfile::tempdir().unwrap();
        write_file(spring.path(), "pom.xml", "<artifactId>spring-webmvc</artifactId>");
        assert_eq!(type_of(spring.path()), ProjectType::Spring);

        let plain = tempfile::tempdir().unwrap();
        write_file(plain.path(), "pom.xml", "<artifactId>jakarta.servlet-api</artifactId>");
        assert_eq!(type_of(plain.path()), ProjectType::Java);

        let gradle = tempfile::tempdir().unwrap();
        write_file(gradle.path(), "build.gradle", "implementation 'spring-boot-starter'");
        assert_eq!(type_of(gradle.path()), ProjectType::SpringBoot);
    }

    /// Java：源目录只是子目录时靠上级构建文件判断；没有构建文件时靠特征文件
    #[test]
    fn detects_java_from_parent_dir_and_markers() {
        let root = tempfile::tempdir().unwrap();
        write_file(root.path(), "pom.xml", "<artifactId>spring-boot-starter</artifactId>");
        let webapp = root.path().join("src/main/webapp");
        fs::create_dir_all(&webapp).unwrap();
        assert_eq!(type_of(&webapp), ProjectType::SpringBoot);

        let legacy = tempfile::tempdir().unwrap();
        write_file(legacy.path(), "src/main/webapp/WEB-INF/web.xml", "<web-app/>");
        assert_eq!(type_of(legacy.path()), ProjectType::Java);

        let struts2 = tempfile::tempdir().unwrap();
        write_file(struts2.path(), "src/main/resources/struts.xml", "<struts/>");
        assert_eq!(type_of(struts2.path()), ProjectType::Struts2);
    }

    /// 其它：layui 仍可识别，都不是则未知
    #[test]
    fn detects_layui_and_unknown() {
        let layui = tempfile::tempdir().unwrap();
        write_file(layui.path(), "js/layui.js", "// layui");
        assert_eq!(type_of(layui.path()), ProjectType::Layui);

        let other = tempfile::tempdir().unwrap();
        write_file(other.path(), "readme.txt", "hi");
        assert_eq!(type_of(other.path()), ProjectType::Unknown);
    }

    /// git config 解析：优先取 origin 的地址
    #[test]
    fn parse_git_config_prefers_origin() {
        let content = "\
[core]
\trepositoryformatversion = 0
[remote \"upstream\"]
\turl = https://example.com/upstream.git
[remote \"origin\"]
\turl = git@example.com:team/app.git
\tfetch = +refs/heads/*:refs/remotes/origin/*
";
        assert_eq!(
            parse_git_config_url(content),
            Some("git@example.com:team/app.git".to_string())
        );
    }

    /// git config 解析：没有 origin 时取第一个远程地址
    #[test]
    fn parse_git_config_falls_back_to_first_remote() {
        let content = "\
[core]
\trepositoryformatversion = 0
[remote \"upstream\"]
\turl = https://example.com/upstream.git
";
        assert_eq!(
            parse_git_config_url(content),
            Some("https://example.com/upstream.git".to_string())
        );
    }

    /// 源目录为仓库子目录时，也能向上找到 git 根目录并读出地址
    #[test]
    fn detect_vcs_finds_parent_git_repo() {
        let root = tempfile::tempdir().unwrap();
        write_file(
            root.path(),
            ".git/config",
            "[remote \"origin\"]\n\turl = git@example.com:team/app.git\n",
        );
        let nested = root.path().join("src/pages");
        fs::create_dir_all(&nested).unwrap();

        let info = detect_vcs(&nested.to_string_lossy());
        assert_eq!(info.vcs_type, VcsType::Git);
        assert_eq!(info.url, "git@example.com:team/app.git");
    }

    /// 非版本控制目录返回 none
    #[test]
    fn detect_vcs_returns_none_for_plain_dir() {
        let dir = tempfile::tempdir().unwrap();
        let info = detect_vcs(&dir.path().to_string_lossy());
        assert_eq!(info.vcs_type, VcsType::None);
        assert!(info.url.is_empty());
    }

    /// 不存在的目录直接返回 none，不 panic
    #[test]
    fn detect_vcs_returns_none_for_missing_dir() {
        let info = detect_vcs("/definitely/not/exists/here");
        assert_eq!(info.vcs_type, VcsType::None);
        assert!(info.url.is_empty());
    }

    /// wc.db 回退解析：取出现次数最多的仓库地址
    #[test]
    fn extract_repo_url_picks_most_frequent() {
        let bytes: &[u8] = b"https://svn.example.com/other/trunk\x00https://svn.example.com/team/app\x00https://svn.example.com/team/app\x00https://svn.example.com/team/app";
        assert_eq!(
            extract_repo_url(bytes),
            Some("https://svn.example.com/team/app".to_string())
        );
    }

    /// wc.db 回退解析：去掉粘连在地址尾部的标点
    #[test]
    fn extract_repo_url_trims_trailing_punctuation() {
        assert_eq!(
            extract_repo_url(b"svn://svn.example.com/team/app,"),
            Some("svn://svn.example.com/team/app".to_string())
        );
    }

    /// wc.db 中没有合法 scheme 时返回 None
    #[test]
    fn extract_repo_url_returns_none_without_scheme() {
        assert_eq!(extract_repo_url(b"/local/path/only"), None);
    }
}
