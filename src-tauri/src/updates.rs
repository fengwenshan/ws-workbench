//! 更新包的断点续传下载与安装。
//!
//! 插件自带的 `Update::download` 把整包读进内存，一旦失败重试就从 0 开始；
//! 这里改成自己发 Range 请求、把数据落盘到
//! `<app_cache_dir>/updater/<版本>-<下载URL哈希>/`，失败保留 `.part`，
//! 下次同版本调用接着下；下完用 minisign 校验签名，通过后落成成品文件，
//! 再交给插件的 `Update::install` 安装。
//!
//! `Update` 由前端 `@tauri-apps/plugin-updater` 的 `check()` 放进 webview 的
//! resource table，因此命令里用 rid 取回它，不重复 check。

use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use base64::Engine as _;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_LENGTH, CONTENT_RANGE, RANGE},
    Client, StatusCode, Url,
};
use serde::Serialize;
use tauri::{ipc::Channel, AppHandle, Manager, ResourceId, Webview};
use tauri_plugin_updater::Update;

/// 缓存目录里的两个文件名：`.part` 是下载中的半成品，另一个是校验通过的成品。
const PART_NAME: &str = "update.part";
const PACKAGE_NAME: &str = "update.pkg";

/// 进度回调节流间隔：下载途中约每 100ms 推一次，避免高频 IPC 反过来拖慢下载。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// 连接/读数据卡住的容忍度。
///
/// 不设总超时（慢速网络也可能下完几分钟），但「30 秒建不起连接」「120 秒收不到
/// 一个字节」必须报错让用户能点重试：`.part` 已经保留，重试就从断点续；
/// 否则连接被中间设备黑洞掉时界面会一直停在 50% 不动。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(120);

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// 同一时刻只允许一个下载任务：两个命令并发会把同一个 `.part` 写花，
/// 断点续传的前提（文件长度就是已下载字节数）也就不成立了。
static DOWNLOADING: AtomicBool = AtomicBool::new(false);

/// 下载进度：`downloaded` 是已完成的总字节数（含上次续传的部分）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
    resumed_from: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDownloadResult {
    version: String,
    /// 校验通过的成品文件绝对路径，安装命令按同样规则算出来再读
    path: String,
    total: u64,
    /// 本次下载从第几个字节接着下的（0 表示从头下）
    resumed_from: u64,
}

/// 断点续传下载更新包。
///
/// 失败（断网、连接中断、写入失败）时 `.part` 保留，用户点重试会从断点继续；
/// 签名校验不过时清空缓存自动从头再下一次，避免坏半成品卡住流程。
#[tauri::command]
pub async fn update_download_resumed(
    app: AppHandle,
    webview: Webview,
    rid: ResourceId,
    version: String,
    on_progress: Channel<UpdateProgress>,
) -> Result<UpdateDownloadResult, String> {
    let update = take_update(&webview, rid, &version)?;
    let pubkey = updater_pubkey(&app)?;
    let dir = package_dir(&app, &update)?;

    let _guard = DownloadGuard::acquire()?;

    let client = build_client(&update)?;
    let signature = update.signature.clone();
    let outcome = download_package(
        &client,
        &update.download_url,
        &update.headers,
        &dir,
        |bytes| verify_signature(bytes, &signature, &pubkey),
        move |progress| {
            let _ = on_progress.send(progress);
        },
    )
    .await?;

    // 新包已就绪，顺手清掉其他版本的缓存，别让旧安装包一直占着磁盘
    cleanup_other_versions(&dir);

    Ok(UpdateDownloadResult {
        version: update.version,
        path: outcome.path.to_string_lossy().into_owned(),
        total: outcome.total,
        resumed_from: outcome.resumed_from,
    })
}

/// 安装已下载并校验过的更新包。
#[tauri::command]
pub async fn update_install_resumed(
    app: AppHandle,
    webview: Webview,
    rid: ResourceId,
    version: String,
) -> Result<(), String> {
    // 开发构建的可执行文件就在 target/debug 下，装更新会把它覆盖掉，
    // 之后 `tauri dev` 的增量构建就乱了，直接拒绝。
    if cfg!(debug_assertions) {
        return Err("开发构建无法安装更新".to_string());
    }

    let update = take_update(&webview, rid, &version)?;
    let pubkey = updater_pubkey(&app)?;
    let dir = package_dir(&app, &update)?;

    let path = dir.join(PACKAGE_NAME);
    let bytes = fs::read(&path).map_err(|err| {
        format!(
            "安装更新失败：更新包不存在，请重新下载（{}：{err}）",
            path.display()
        )
    })?;
    // 下载时校验过，但缓存文件在两次命令之间可能被改动，安装前再校验一次
    verify_signature(&bytes, &update.signature, &pubkey)
        .map_err(|err| format!("校验更新包失败：{err}"))?;

    // macOS 上要解压 tar.gz 并替换 .app，Windows 上要写安装包再拉起，
    // 都是阻塞操作，别占着 tokio 的工作线程。
    tauri::async_runtime::spawn_blocking(move || update.install(&bytes))
        .await
        .map_err(|err| format!("安装更新失败：{err}"))?
        .map_err(|err| format!("安装更新失败：{err}"))
}

/// 从 webview 的 resource table 取回前端 check() 拿到的 `Update`。
fn take_update(webview: &Webview, rid: ResourceId, version: &str) -> Result<Update, String> {
    let update = webview
        .resources_table()
        .get::<Update>(rid)
        .map_err(|err| format!("无法取回更新信息：{err}"))?;
    // 克隆一份：下载/安装可能耗时几十秒，不能一直占着资源表的锁，
    // 期间 JS 侧释放 rid 也不该影响已经开始的流程。
    let update = (*update).clone();
    if update.version != version {
        return Err(format!(
            "无法取回更新信息：版本不一致（界面 {version}，实际 {}）",
            update.version
        ));
    }
    Ok(update)
}

/// 更新公钥来自 tauri.conf.json 的 `plugins.updater.pubkey`（发布时注入），
/// 不在代码里写死，换密钥不用改 Rust。
fn updater_pubkey(app: &AppHandle) -> Result<String, String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("pubkey"))
        .and_then(|pubkey| pubkey.as_str())
        .filter(|pubkey| !pubkey.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "无法取回更新信息：tauri.conf.json 未配置 updater.pubkey".to_string())
}

/// 某个版本更新包的缓存目录：`<cache>/updater/<版本>-<下载URL哈希>/`。
///
/// URL 一起进目录名，是因为同一个版本号可能来自不同下载源（例如镜像切换），
/// 把 A 源的半成品续到 B 源上只会下出一个坏包。
fn package_dir(app: &AppHandle, update: &Update) -> Result<PathBuf, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|err| format!("无法取回更新信息：{err}"))?;
    let mut hasher = DefaultHasher::new();
    update.download_url.as_str().hash(&mut hasher);
    let dir_name = format!(
        "{}-{:016x}",
        sanitize_version(&update.version),
        hasher.finish()
    );
    Ok(cache_dir.join("updater").join(dir_name))
}

/// 版本号来自更新清单 JSON，理论上可能带路径分隔符，落目录名前过滤一道。
fn sanitize_version(version: &str) -> String {
    version
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// 下载成功一个版本后清理其他版本的缓存目录（best-effort，失败不影响本次更新）。
fn cleanup_other_versions(current: &Path) {
    let Some(root) = current.parent() else {
        return;
    };
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path != current && path.is_dir() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

/// 请求客户端：沿用插件 `Update` 里的代理配置，行为跟插件下载基本一致，
/// 只额外加了连接/读数据超时（见 `CONNECT_TIMEOUT` / `READ_TIMEOUT`）。
///
/// 不能用 `update.timeout` 当整包下载的总超时：那是前端 `check({ timeout })`
/// 给「拿清单」设的（十几秒），套到 15MB 的安装包上会把正常慢速下载掐断。
/// 卡死靠 connect_timeout / read_timeout 兜，断点续传保证重试不从头来。
fn build_client(update: &Update) -> Result<Client, String> {
    ensure_crypto_provider();
    let mut builder = Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT);
    if update.no_proxy {
        builder = builder.no_proxy();
    } else if let Some(proxy) = &update.proxy {
        let proxy = reqwest::Proxy::all(proxy.as_str())
            .map_err(|err| format!("下载更新失败：代理配置无效：{err}"))?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|err| format!("下载更新失败：创建 HTTP 客户端失败：{err}"))
}

/// reqwest 是用 rustls 的 no-provider 特性编译的（和 updater 插件一致），
/// 构建客户端前必须装好 crypto provider，否则会 panic。
fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// 同一时刻只允许一个下载任务的守卫，drop 时自动释放。
struct DownloadGuard;

impl DownloadGuard {
    fn acquire() -> Result<Self, String> {
        if DOWNLOADING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("下载更新失败：已有一个更新包正在下载".to_string());
        }
        Ok(Self)
    }
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        DOWNLOADING.store(false, Ordering::SeqCst);
    }
}

/// 缓存里已落盘的成品信息。
#[derive(Debug)]
struct PackageInfo {
    path: PathBuf,
    total: u64,
    resumed_from: u64,
}

/// 把更新包下到缓存目录，返回成品路径。签名校验由调用方注入，测试里可以换成
/// 内容比对，不必造一对真实的 minisign 密钥。
async fn download_package<F, V>(
    client: &Client,
    url: &Url,
    headers: &HeaderMap,
    dir: &Path,
    verify: V,
    mut report: F,
) -> Result<PackageInfo, String>
where
    F: FnMut(UpdateProgress),
    V: Fn(&[u8]) -> Result<(), String>,
{
    fs::create_dir_all(dir).map_err(|err| format!("下载更新失败：创建缓存目录失败：{err}"))?;
    let package_path = dir.join(PACKAGE_NAME);
    let part_path = dir.join(PART_NAME);

    // 上次已经下完、只是安装失败（或用户没重启）时，成品文件还在，直接复用
    if package_path.exists() {
        if let Ok(bytes) = fs::read(&package_path) {
            if verify(&bytes).is_ok() {
                let total = bytes.len() as u64;
                report(UpdateProgress {
                    downloaded: total,
                    total: Some(total),
                    resumed_from: 0,
                });
                return Ok(PackageInfo {
                    path: package_path,
                    total,
                    resumed_from: 0,
                });
            }
        }
        // 成品对不上：缓存被改过或远端换了包，删掉重下
        let _ = fs::remove_file(&package_path);
    }

    // 校验不过说明 `.part` 和远端内容对不上，清空后从头再来一次；第二次还失败才报错，
    // 避免用户只看到「校验失败」却怎么点都没用。
    let mut attempts_left = 2;
    loop {
        attempts_left -= 1;
        let start_offset = fs::metadata(&part_path).map(|meta| meta.len()).unwrap_or(0);
        let outcome =
            fetch_package(client, url, headers, &part_path, start_offset, &mut report).await?;
        let part_len = fs::metadata(&part_path).map(|meta| meta.len()).unwrap_or(0);

        let resumed_from = match outcome {
            FetchOutcome::Downloaded { resumed_from } => resumed_from,
            // 416：服务端认为 Range 起点越界，本地 `.part` 可能已经是完整包，
            // 是否真的完整交给签名判断；大小都对不上就直接删掉重下。
            FetchOutcome::NotSatisfiable { total } => {
                if total.is_some_and(|total| total != part_len) {
                    let _ = fs::remove_file(&part_path);
                    if attempts_left == 0 {
                        return Err("下载更新失败：本地缓存与远端更新包大小不一致".to_string());
                    }
                    continue;
                }
                0
            }
        };

        if part_len == 0 {
            return Err("下载更新失败：服务器返回的更新包为空".to_string());
        }

        let bytes = match fs::read(&part_path) {
            Ok(bytes) => bytes,
            Err(err) => {
                let _ = fs::remove_file(&part_path);
                if attempts_left == 0 {
                    return Err(format!("下载更新失败：读取缓存文件失败：{err}"));
                }
                continue;
            }
        };

        match verify(&bytes) {
            Ok(()) => {
                fs::rename(&part_path, &package_path)
                    .map_err(|err| format!("下载更新失败：写入缓存文件失败：{err}"))?;
                report(UpdateProgress {
                    downloaded: part_len,
                    total: Some(part_len),
                    resumed_from,
                });
                return Ok(PackageInfo {
                    path: package_path,
                    total: part_len,
                    resumed_from,
                });
            }
            Err(err) => {
                let _ = fs::remove_file(&part_path);
                if attempts_left == 0 {
                    return Err(format!("校验更新包失败：{err}"));
                }
            }
        }
    }
}

/// 单次下载尝试的结果。
enum FetchOutcome {
    /// 请求正常结束，`.part` 里是完整的候选包（是否正确由签名校验决定）
    Downloaded { resumed_from: u64 },
    /// 服务端对 Range 返回 416：本地 `.part` 已经不小于远端总大小
    NotSatisfiable { total: Option<u64> },
}

/// 发请求并把响应写进 `.part`。服务端可能不认 Range，所以最多重来一次。
async fn fetch_package<F>(
    client: &Client,
    url: &Url,
    base_headers: &HeaderMap,
    part_path: &Path,
    start_offset: u64,
    report: &mut F,
) -> Result<FetchOutcome, String>
where
    F: FnMut(UpdateProgress),
{
    let mut offset = start_offset;
    for _ in 0..2 {
        match request_into_part(client, url, base_headers, part_path, offset, report).await? {
            Attempt::Done { resumed_from } => {
                return Ok(FetchOutcome::Downloaded { resumed_from });
            }
            // 服务端返回的范围起点对不上，丢掉这次响应，从头再请求一次
            Attempt::RangeMismatch => offset = 0,
            Attempt::NotSatisfiable { total } => {
                return Ok(FetchOutcome::NotSatisfiable { total });
            }
        }
    }
    Err("下载更新失败：服务器返回的续传范围与请求不一致".to_string())
}

enum Attempt {
    Done { resumed_from: u64 },
    RangeMismatch,
    NotSatisfiable { total: Option<u64> },
}

/// 发一次 GET（带 Range）并把响应体追加/覆盖进 `.part`。
///
/// 任何错误都在 flush 之后才返回，保证 `.part` 的长度就是下次可以续传的起点。
async fn request_into_part<F>(
    client: &Client,
    url: &Url,
    base_headers: &HeaderMap,
    part_path: &Path,
    offset: u64,
    report: &mut F,
) -> Result<Attempt, String>
where
    F: FnMut(UpdateProgress),
{
    let mut headers = base_headers.clone();
    if !headers.contains_key(ACCEPT) {
        headers.insert(ACCEPT, HeaderValue::from_static("application/octet-stream"));
    }
    if offset > 0 {
        let range = HeaderValue::from_str(&format!("bytes={offset}-"))
            .map_err(|err| format!("下载更新失败：Range 请求头无效：{err}"))?;
        headers.insert(RANGE, range);
    }

    let mut response = client
        .get(url.clone())
        .headers(headers)
        .send()
        .await
        .map_err(|err| format!("下载更新失败：{err}"))?;

    let status = response.status();
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        let total = header_str(response.headers(), CONTENT_RANGE)
            .and_then(parse_content_range)
            .and_then(|range| range.total);
        return Ok(Attempt::NotSatisfiable { total });
    }
    if !status.is_success() {
        return Err(format!("下载更新失败：服务器返回 HTTP {}", status.as_u16()));
    }

    // 只有 206 且响应起点正好是本地已下长度时才能追加；200 表示服务端忽略了
    // Range，返回的是整包，只能清空已有内容从头写。
    let (write_offset, total) = if status == StatusCode::PARTIAL_CONTENT {
        let range = header_str(response.headers(), CONTENT_RANGE).and_then(parse_content_range);
        match range {
            Some(range) if range.start == Some(offset) => (offset, range.total),
            _ => return Ok(Attempt::RangeMismatch),
        }
    } else {
        let total = header_str(response.headers(), CONTENT_LENGTH)
            .and_then(|value| value.parse::<u64>().ok());
        (0, total)
    };

    let mut file = if write_offset == 0 {
        File::create(part_path).map_err(|err| format!("下载更新失败：创建缓存文件失败：{err}"))?
    } else {
        OpenOptions::new()
            .append(true)
            .create(true)
            .open(part_path)
            .map_err(|err| format!("下载更新失败：打开缓存文件失败：{err}"))?
    };

    let mut reporter = ProgressReporter::new(report, write_offset);
    // 拿到响应头就先推一次，前端能把进度条从续传起点画出来
    reporter.emit(write_offset, total, true);

    let mut downloaded = write_offset;
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(err) => {
                // 已写入的字节就是下次续传的起点，保留 .part
                let _ = file.flush();
                return Err(format!("下载更新失败：{err}"));
            }
        };
        if let Err(err) = file.write_all(&chunk) {
            let _ = file.flush();
            return Err(format!("下载更新失败：写入缓存文件失败：{err}"));
        }
        downloaded += chunk.len() as u64;
        reporter.emit(downloaded, total, false);
    }
    file.flush()
        .map_err(|err| format!("下载更新失败：写入缓存文件失败：{err}"))?;
    drop(file);

    // 有 Content-Length / Content-Range 时校验长度：服务端正常关闭连接但少发数据
    // （比如中间有代理）时不会报错，只能靠自己数。
    if let Some(total) = total {
        if downloaded < total {
            return Err(format!(
                "下载更新失败：连接提前中断（已下载 {downloaded} / {total} 字节）"
            ));
        }
    }

    reporter.emit(downloaded, total, true);
    Ok(Attempt::Done {
        resumed_from: write_offset,
    })
}

/// 进度节流器：拿到响应头、下载结束各强制推一次，中间约每 100ms 推一次。
struct ProgressReporter<'a, F: FnMut(UpdateProgress)> {
    report: &'a mut F,
    last: Instant,
    resumed_from: u64,
}

impl<'a, F: FnMut(UpdateProgress)> ProgressReporter<'a, F> {
    fn new(report: &'a mut F, resumed_from: u64) -> Self {
        Self {
            report,
            last: Instant::now(),
            resumed_from,
        }
    }

    fn emit(&mut self, downloaded: u64, total: Option<u64>, force: bool) {
        if force || self.last.elapsed() >= PROGRESS_INTERVAL {
            self.last = Instant::now();
            (self.report)(UpdateProgress {
                downloaded,
                total,
                resumed_from: self.resumed_from,
            });
        }
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: HeaderName) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

/// `Content-Range` 里能拿到的信息。
struct ContentRange {
    /// 本次响应体的起始字节；`bytes */total`（416）这种形式没有起点
    start: Option<u64>,
    /// 远端文件总大小；`*` 表示未知
    total: Option<u64>,
}

/// 解析 `Content-Range: bytes 100-199/200` 或 `bytes */200`。
///
/// 起点决定能不能往已有 `.part` 上追加，总大小用来画进度和判断是否下完。
fn parse_content_range(value: &str) -> Option<ContentRange> {
    let mut parts = value.split_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range, total) = parts.next()?.split_once('/')?;

    let total = total.trim();
    let total = if total == "*" {
        None
    } else {
        Some(total.parse::<u64>().ok()?)
    };

    let range = range.trim();
    let start = if range == "*" {
        None
    } else {
        let (start, end) = range.split_once('-')?;
        // end 只做格式校验，完整性用总大小判断
        end.trim().parse::<u64>().ok()?;
        Some(start.trim().parse::<u64>().ok()?)
    };

    Some(ContentRange { start, total })
}

/// 校验更新包签名：pubkey 和 signature 都是 base64 包着一整段 minisign 文本，
/// 和插件 `verify_signature` 的处理完全一致。
fn verify_signature(data: &[u8], signature: &str, pubkey: &str) -> Result<(), String> {
    let pubkey = decode_base64_text(pubkey).map_err(|err| format!("公钥解码失败：{err}"))?;
    let public_key = minisign_verify::PublicKey::decode(&pubkey)
        .map_err(|err| format!("公钥格式无效：{err}"))?;
    let signature = decode_base64_text(signature).map_err(|err| format!("签名解码失败：{err}"))?;
    let signature = minisign_verify::Signature::decode(&signature)
        .map_err(|err| format!("签名格式无效：{err}"))?;
    public_key
        .verify(data, &signature, true)
        .map_err(|err| format!("签名不匹配：{err}"))
}

fn decode_base64_text(value: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|err| err.to_string())?;
    String::from_utf8(bytes).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{Shutdown, TcpListener, TcpStream},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        thread,
    };

    /// 本机 mock HTTP 服务：`responder` 拿请求原文和连接序号，返回要写回的响应字节。
    /// 返回后服务端立刻关闭写方向，用短 body + 大 Content-Length 就能模拟「下到一半断线」。
    fn spawn_mock_server<F>(responder: F) -> String
    where
        F: Fn(&str, usize) -> Vec<u8> + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("mock server addr");
        let served = Arc::new(AtomicUsize::new(0));
        let served_in_server = served.clone();
        let responder = Arc::new(responder);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let request = read_request(&mut stream);
                let index = served_in_server.fetch_add(1, Ordering::SeqCst);
                let response = responder(&request, index);
                let _ = stream.write_all(&response);
                let _ = stream.flush();
                let _ = stream.shutdown(Shutdown::Write);
            }
        });
        format!("http://{addr}/ws-workbench.zip")
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    buf.extend_from_slice(&chunk[..read]);
                    if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn response(status: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
        let mut head = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(body);
        out
    }

    /// 可复现的假更新包内容（不依赖随机数，失败时好对比）
    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index % 251) as u8).collect()
    }

    #[tokio::test]
    async fn resumes_after_connection_drop() {
        ensure_crypto_provider();
        const TOTAL: usize = 700 * 1024;
        const FIRST: usize = 300 * 1024;

        let content = Arc::new(payload(TOTAL));
        let content_in_server = content.clone();
        let url = spawn_mock_server(move |request, index| {
            if index == 0 {
                // 只发 300KB 就断：Content-Length 声明的是整包，客户端会收到连接中断
                response(
                    "200 OK",
                    &[("Content-Length", content_in_server.len().to_string())],
                    &content_in_server[..FIRST],
                )
            } else {
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains(&format!("range: bytes={FIRST}-")),
                    "第二个请求应带上断点 Range 头，实际请求：{request}"
                );
                let rest = &content_in_server[FIRST..];
                response(
                    "206 Partial Content",
                    &[
                        ("Content-Length", rest.len().to_string()),
                        (
                            "Content-Range",
                            format!(
                                "bytes {FIRST}-{}/{}",
                                content_in_server.len() - 1,
                                content_in_server.len()
                            ),
                        ),
                    ],
                    rest,
                )
            }
        });
        let url = Url::parse(&url).expect("mock url");
        let client = Client::builder().build().expect("test client");
        let temp = tempfile::tempdir().expect("temp dir");
        let dir = temp.path().join("updater");

        let verifier = |bytes: &[u8]| {
            if bytes == content.as_slice() {
                Ok(())
            } else {
                Err("内容不一致".to_string())
            }
        };

        // 第一次：连接在中途断开，必须报错且 .part 保留 300KB
        let first_error =
            download_package(&client, &url, &HeaderMap::new(), &dir, verifier, |_| {})
                .await
                .expect_err("第一次下载应当失败");
        assert!(
            first_error.starts_with("下载更新失败："),
            "错误文案应以「下载更新失败：」开头，实际：{first_error}"
        );
        let part_path = dir.join(PART_NAME);
        assert_eq!(
            fs::metadata(&part_path).expect(".part 应保留").len(),
            FIRST as u64
        );
        assert!(!dir.join(PACKAGE_NAME).exists());

        // 第二次：带 Range 续传，只补剩下的 400KB，内容完整
        let mut progress = Vec::new();
        let result = download_package(
            &client,
            &url,
            &HeaderMap::new(),
            &dir,
            |bytes: &[u8]| {
                if bytes == content.as_slice() {
                    Ok(())
                } else {
                    Err("内容不一致".to_string())
                }
            },
            |event| progress.push(event),
        )
        .await
        .expect("第二次下载应成功");

        assert_eq!(result.resumed_from, FIRST as u64);
        assert_eq!(result.total, TOTAL as u64);
        assert_eq!(fs::read(&result.path).expect("成品文件"), *content);
        assert!(!part_path.exists(), "完成后 .part 应被改名成成品");
        assert_eq!(
            progress.first().map(|event| event.resumed_from),
            Some(FIRST as u64)
        );
        assert_eq!(
            progress.last().map(|event| event.downloaded),
            Some(TOTAL as u64)
        );
    }

    #[tokio::test]
    async fn restarts_from_zero_when_server_ignores_range() {
        ensure_crypto_provider();
        let content = Arc::new(payload(64 * 1024));
        let content_in_server = content.clone();
        let url = spawn_mock_server(move |_request, _index| {
            // 不管有没有 Range 都回整包（200），模拟不支持断点续传的服务器
            response(
                "200 OK",
                &[("Content-Length", content_in_server.len().to_string())],
                &content_in_server,
            )
        });
        let url = Url::parse(&url).expect("mock url");
        let client = Client::builder().build().expect("test client");
        let temp = tempfile::tempdir().expect("temp dir");
        let dir = temp.path().join("updater");

        // 先放一个上次下了一半的 .part（内容是垃圾，模拟半成品损坏）
        fs::create_dir_all(&dir).expect("缓存目录");
        fs::write(dir.join(PART_NAME), vec![7u8; 100]).expect("预置 .part");

        let mut progress = Vec::new();
        let result = download_package(
            &client,
            &url,
            &HeaderMap::new(),
            &dir,
            |bytes: &[u8]| {
                if bytes == content.as_slice() {
                    Ok(())
                } else {
                    Err("内容不一致".to_string())
                }
            },
            |event| progress.push(event),
        )
        .await
        .expect("服务端忽略 Range 时应从头重下并成功");

        assert_eq!(result.resumed_from, 0);
        assert_eq!(result.total, content.len() as u64);
        assert_eq!(fs::read(&result.path).expect("成品文件"), *content);
        assert_eq!(progress.first().map(|event| event.resumed_from), Some(0));
    }

    #[test]
    fn parses_content_range() {
        let cases = [
            ("bytes 0-99/100", Some((Some(0), Some(100)))),
            ("bytes 100-199/200", Some((Some(100), Some(200)))),
            ("bytes 100-199/*", Some((Some(100), None))),
            ("bytes */12345", Some((None, Some(12345)))),
            ("bytes 0-0/1", Some((Some(0), Some(1)))),
            // 大小写、多空格、首尾空白都按 HTTP 语义容忍
            ("BYTES   0-99/100  ", Some((Some(0), Some(100)))),
            ("bytes 0-99", None),
            ("bytes 0-99/x", None),
            ("bytes abc/100", None),
            ("items 0-99/100", None),
            ("", None),
        ];
        for (value, expected) in cases {
            let parsed = parse_content_range(value).map(|range| (range.start, range.total));
            assert_eq!(parsed, expected, "解析 {value:?} 出错");
        }
    }
}
