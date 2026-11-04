# 笔记编辑器升级：md-editor-v3 + 图片加密内联 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用成熟开源库 `md-editor-v3` 替换现有「6 个手写按钮 + 纯 `Textarea` + `marked` 预览」的笔记编辑器，提供 Markdown 源码 + 实时预览、目录（TOC）大纲、表格/任务列表等 GFM 语法、代码块高亮，并让图片可**粘贴 / 拖拽 / 选择文件**上传——密文（`.wst`）落盘、预览内联显示、复用现有附件体系。

**Architecture:** 前端把主编辑器换成 `md-editor-v3` 的 `<MdEditor>`，通过其 `onUploadImg` 钩子接管图片：`FileReader` → base64 → 新增命令 `encrypt_upload_bytes`（内存加密，全程无明文临时文件）→ 原子写入 `<upload_dir>/<uuid>.wst` → 返回 `AttachmentMeta` 入附件列表 → 以 `wst-img` 自定义 URI 协议回显预览。后端复用 `WST1` 帧格式（SM4-CBC + HMAC-SM3），仅把流式加解密的读写循环抽成对 `Read`/`Write` 泛型的核心函数，再在核心之上加两个纯内存入口；不新增密钥、不改配置加密信封边界、不改 `note: string` 数据类型。

**Tech Stack:** Tauri 2.11.5 · Rust 2021（`sm4`/`cbc`/`sm3`/`hmac`/`rand`/`base64`/`uuid`/`tempfile`）· Vue 3.5.42 · TypeScript 5.6 · Vite 6 · md-editor-v3 `^7.1.0` · ant-design-vue 4（按需导入）。

**关联设计文档:** `docs/superpowers/specs/2026-09-30-note-editor-md-editor-v3-design.md`

**约定：**
- Rust 命令在 `src-tauri/` 下执行；前端命令在仓库根目录执行。
- 校验命令：`cargo test` / `cargo check`（`src-tauri/` 下）、`pnpm build`（`vue-tsc --noEmit && vite build`）。
- 每个任务末尾的 `git commit` 步骤**仅在用户明确同意提交后执行**；未获同意时跳过提交、保留工作区改动。
- 每个任务完成后按项目规则重启应用（见 Task 8）。重启用 `pnpm exec tauri dev`，**不要**用 `pnpm tauri dev`（`package.json` 里的 `tauri` 脚本会把命令解析成 `tauri dev dev`）。

---

## 实施现状（执行前必读）

下列事实均已在本工作区逐项核对（分支 `feat/vpn`），据此写成下文可直达的步骤：

- **`md-editor-v3` 尚未安装**；`vue` 实装版本 `3.5.42`（满足其 `peerDependencies: vue ^3.5.3`）。因此 Task 4 必须先安装。
- **`marked` / `dompurify` 全项目仅 `NoteEditor.vue` 引用**（`src/components/NoteEditor.vue:4,5,47,48`）→ 重写后可安全移除（Task 7）。
- **`base64 = "0.22"` 已是 `Cargo.toml` 依赖**（`src-tauri/Cargo.toml:28`）→ 后端只需在新命令里 `use`，**无需改清单**。
- **当前未注册任何自定义 URI 协议**；`tauri.conf.json` 的 `csp: null`、`dragDropEnabled: false` 保持不变，正是本特性所需（无 CSP 拦截、拖拽/粘贴事件直达 webview）。
- **`App.vue` 已完整接线** `@update-note` / `@add-attachment` / `@log`（`src/App.vue:709-714`、`725-730`）与 `handleEntityNote` / `handleEntityAddAttachment`（`src/App.vue:447`、`459`）→ 只需改两个 `*Detail.vue`，**上层无需改动**。
- **`useConfig.addAttachment(kind, id, meta)` 已存在**（`src/composables/useConfig.ts:225`）→ 图片入列表即复用现有落盘链路。
- **`get_platform_info` → `PlatformInfo{ is_windows, ... }`**（`src-tauri/src/platform.rs`、`src/platform.ts:80` `isWindows()`）→ 前端据此选协议前缀，无需新增命令。
- **无 JS 测试运行器**：前端以 `pnpm build`（`vue-tsc --noEmit && vite build`）+ 重启实机验收；后端用 `cargo test`。

### 关键前置：md-editor-v3 `^7.1.0` 已核实 API（来自其 `lib/types/index.d.ts`）

- `toolbars: ToolbarNames[]`，`type ToolbarNames = keyof ToolbarTips | number`。**7.1.0 的合法 key 白名单**（与旧版本不同，勿凭记忆）：`bold underline italic strikeThrough title sub sup quote unorderedList orderedList task codeRow code link image table mermaid katex revoke next save prettier pageFullscreen fullscreen preview previewOnly htmlPreview catalog github '-' '='`。
- `onUploadImg` 是**prop（回调）**，不是 emit：`(files: File[], callBack: (urls: string[]) => void) => void`。模板用 `:on-upload-img="fn"`（不可写 `@on-upload-img`）。
- 相关 props：`language`（`'zh-CN'`）、`theme`（`'light' | 'dark'`）、`previewTheme`（`string`）、`codeTheme`（string）、`showCodeRowNumber`、`noMermaid`、`noKatex`、`toolbars`。
- 用法：`import { MdEditor } from 'md-editor-v3'` + `import 'md-editor-v3/lib/style.css'`。

### 待实现时确认项（≤ 验收前必须落实）

1. **离线代码高亮（重要）**：md-editor-v3 `^7.1.0` 默认从 `https://unpkg.com` 拉取 highlight.js / prettier / mermaid / katex / 代码主题 CSS。本应用是桌面工具，离线或内网环境会导致代码高亮与代码主题失效。缓解路径（本计划**不默认实施**，除非验收发现离线异常再启用）：`pnpm add highlight.js` 后在前端入口 `config({ editorExtensions: { highlight: { instance: hljs } } })`（`instance?: any`）。见 Self-Review §4。
2. md-editor-v3 内部 CSS 类名/变量（`.md-editor`、`.md-editor-toolbar-wrapper`、`--md-bk-color` 等）以安装后实际渲染为准；本计划只做「撑满高度 + 边框 + 少量令牌覆盖」，视觉细节在 Task 8 实机确认并微调。
3. `previewTheme="github"` / `codeTheme="github"` 的代码主题 CSS 亦来自 CDN（同上第 1 条）。

---

## 文件结构

| 文件 | 动作 | 职责 |
| --- | --- | --- |
| `src-tauri/src/filecrypto.rs` | 改 | 抽出 `encrypt_core`/`decrypt_core`（对 `Read`/`Write` 泛型），`encrypt_file_stream`/`decrypt_file_stream` 改为薄封装；新增 `encrypt_bytes`/`decrypt_bytes` 内存入口 + 单测 |
| `src-tauri/src/commands.rs` | 改 | 新增命令 `encrypt_upload_bytes`；新增协议响应助手 `serve_note_image` 及查询串/百分号解码工具 |
| `src-tauri/src/lib.rs` | 改 | 注册 `wst-img` 自定义 URI 协议；`invoke_handler` 挂载 `encrypt_upload_bytes` |
| `package.json` | 改 | 新增 `md-editor-v3`；移除 `marked` / `dompurify` |
| `src/components/NoteEditor.vue` | 重写 | 用 `<MdEditor>` 包装；`onUploadImg` 上传钩子；防抖保存；协议 URL 拼接 |
| `src/components/ProjectDetail.vue` | 改 | 向 `<NoteEditor>` 补 `uploadDir`/`defaultUploadDir` 与 `@add-attachment`/`@log` |
| `src/components/GroupDetail.vue` | 改 | 同上（分组实体） |

---

## Task 1: `filecrypto.rs` 抽核心 + 内存加解密

**Files:**
- Modify: `src-tauri/src/filecrypto.rs`

- [ ] **Step 1: 补 `VaultKeys` 导入**

把

```rust
use crate::crypto::vault_keys;
```

改为

```rust
use crate::crypto::{vault_keys, VaultKeys};
```

- [ ] **Step 2: 新增核心函数 `encrypt_core` / `decrypt_core`**

在 `init_mac` 之后、`encrypt_file_stream` 之前插入。二者与流式入口共用同一帧格式，区别只是泛型化了对 `Read`/`Write` 的依赖，从而既能服务文件、也能服务内存：

```rust
/// 核心加密：把 `reader` 全部内容按 `WST1` 帧格式写入 `writer`
///
/// `orig_size` 为原始字节数，写入文件头并参与 MAC；
/// `on_progress(done, total)` 在每帧写完后回调。
fn encrypt_core<R: Read, W: Write>(
    keys: &VaultKeys,
    reader: &mut R,
    writer: &mut W,
    orig_size: u64,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let mut mac = init_mac(&keys.mac_key)?;

    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.push(VERSION);
    header.push(FLAGS);
    header.extend_from_slice(&orig_size.to_le_bytes());
    writer
        .write_all(&header)
        .map_err(|e| format!("写入文件头失败: {}", e))?;
    mac.update(&header);

    let mut written: u64 = 0;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = read_chunk(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        let mut iv = [0u8; IV_LEN];
        rand::thread_rng().fill_bytes(&mut iv);

        let cipher = Sm4CbcEnc::new_from_slices(&keys.enc_key[..16], &iv)
            .map_err(|e| format!("初始化 SM4 失败: {}", e))?
            .encrypt_padded_vec_mut::<Pkcs7>(&buf[..n]);

        let len_bytes = (n as u32).to_le_bytes();
        writer
            .write_all(&len_bytes)
            .and_then(|_| writer.write_all(&iv))
            .and_then(|_| writer.write_all(&cipher))
            .map_err(|e| format!("写入加密数据失败: {}", e))?;

        mac.update(&len_bytes);
        mac.update(&iv);
        mac.update(&cipher);

        written += n as u64;
        on_progress(written, orig_size);
    }

    // 结束标记（不纳入 MAC：MAC 只覆盖 header ‖ frames）
    writer
        .write_all(&0u32.to_le_bytes())
        .map_err(|e| format!("写入结束标记失败: {}", e))?;

    let tag = mac.finalize().into_bytes();
    writer
        .write_all(&tag)
        .map_err(|e| format!("写入校验码失败: {}", e))?;
    writer.flush().map_err(|e| format!("刷新目标失败: {}", e))?;

    Ok(())
}

/// 核心解密：从 `reader` 读取 `WST1`，明文写入 `writer`，末尾校验 HMAC-SM3
///
/// 校验失败返回 `Err`；调用方据此丢弃已写入的内容（文件场景丢弃临时文件）。
fn decrypt_core<R: Read, W: Write>(
    keys: &VaultKeys,
    reader: &mut R,
    writer: &mut W,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let mut mac = init_mac(&keys.mac_key)?;

    let mut header = [0u8; HEADER_LEN];
    read_exact(reader, &mut header, "文件头")?;
    if &header[0..4] != MAGIC {
        return Err("不是有效的 WST1 文件".to_string());
    }
    if header[4] != VERSION {
        return Err(format!("不支持的 WST1 版本: {}", header[4]));
    }
    mac.update(&header);

    let mut orig = [0u8; 8];
    orig.copy_from_slice(&header[6..14]);
    let total = u64::from_le_bytes(orig);

    let mut written: u64 = 0;
    loop {
        let mut len_bytes = [0u8; 4];
        read_exact(reader, &mut len_bytes, "帧长度")?;
        let plain_len = u32::from_le_bytes(len_bytes) as usize;
        if plain_len == 0 {
            break;
        }

        let mut iv = [0u8; IV_LEN];
        read_exact(reader, &mut iv, "帧 IV")?;

        let cipher_len = (plain_len / BLOCK + 1) * BLOCK;
        let mut cipher = vec![0u8; cipher_len];
        read_exact(reader, &mut cipher, "帧密文")?;

        mac.update(&len_bytes);
        mac.update(&iv);
        mac.update(&cipher);

        let plain = Sm4CbcDec::new_from_slices(&keys.enc_key[..16], &iv)
            .map_err(|e| format!("初始化 SM4 失败: {}", e))?
            .decrypt_padded_vec_mut::<Pkcs7>(&cipher)
            .map_err(|_| "解密失败（填充不合法）".to_string())?;
        if plain.len() != plain_len {
            return Err("密文长度不匹配".to_string());
        }
        writer
            .write_all(&plain)
            .map_err(|e| format!("写入明文失败: {}", e))?;

        written += plain_len as u64;
        on_progress(written, total);
    }

    writer
        .flush()
        .map_err(|e| format!("刷新明文失败: {}", e))?;

    let mut tag = [0u8; MAC_LEN];
    read_exact(reader, &mut tag, "校验码")?;
    mac.verify_slice(&tag)
        .map_err(|_| "文件损坏或无法解密".to_string())?;

    Ok(())
}
```

- [ ] **Step 3: `encrypt_file_stream` 改为薄封装**

整体替换该函数（签名不变，行为不变）：

```rust
/// 流式加密文件：`src` → `dst`（`WST1` 格式）
///
/// `on_progress(done, total)` 在每帧写完后回调，用于推送进度事件。
pub fn encrypt_file_stream(
    app: &AppHandle,
    src: &Path,
    dst: &Path,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let keys = vault_keys(app)?;

    let orig_size = std::fs::metadata(src)
        .map_err(|e| format!("读取源文件信息失败: {}", e))?
        .len();

    let file = File::open(src).map_err(|e| format!("打开源文件失败: {}", e))?;
    let mut reader = BufReader::with_capacity(CHUNK, file);
    let out = File::create(dst).map_err(|e| format!("创建目标文件失败: {}", e))?;
    let mut writer = BufWriter::with_capacity(CHUNK, out);

    encrypt_core(&keys, &mut reader, &mut writer, orig_size, on_progress)
}
```

- [ ] **Step 4: `decrypt_file_stream` 改为薄封装**

整体替换该函数（签名不变；仍「边解密写临时文件 → 校验通过再落位」，失败丢弃临时文件）：

```rust
/// 流式解密 `WST1` 文件：`src` → `dst`
///
/// 边读边解密写入同目录临时文件，末尾 MAC 校验通过后再落位，
/// 校验失败则丢弃临时文件，绝不产出损坏内容。
pub fn decrypt_file_stream(
    app: &AppHandle,
    src: &Path,
    dst: &Path,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let keys = vault_keys(app)?;

    let file = File::open(src).map_err(|e| format!("打开密文失败: {}", e))?;
    let mut reader = BufReader::with_capacity(CHUNK, file);

    let parent = dst
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let tmp = tempfile::NamedTempFile::new_in(&parent)
        .map_err(|e| format!("创建临时文件失败: {}", e))?;
    let mut writer = BufWriter::with_capacity(CHUNK, tmp.as_file());

    decrypt_core(&keys, &mut reader, &mut writer, on_progress)?;
    drop(writer);

    // 目标已存在时先移除，保证 persist 可覆盖（Windows 上 rename 不覆盖）
    match std::fs::remove_file(dst) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("写入目标文件失败: {}", e)),
    }
    tmp.persist(dst)
        .map_err(|e| format!("写入目标文件失败: {}", e))?;

    Ok(())
}
```

- [ ] **Step 5: 新增内存加解密入口**

在 `decrypt_file_stream` 之后追加。`*_with_keys` 为可测的纯函数核心；对外的 `encrypt_bytes`/`decrypt_bytes` 只负责取密钥：

```rust
/// 内存加密：明文 `data` → `WST1` 密文字节（全程不落盘，无明文临时文件）
pub fn encrypt_bytes(app: &AppHandle, data: &[u8]) -> Result<Vec<u8>, String> {
    let keys = vault_keys(app)?;
    encrypt_bytes_with_keys(&keys, data)
}

/// 内存解密：`WST1` 密文字节 → 明文（末尾 HMAC-SM3 校验失败即报错）
pub fn decrypt_bytes(app: &AppHandle, blob: &[u8]) -> Result<Vec<u8>, String> {
    let keys = vault_keys(app)?;
    decrypt_bytes_with_keys(&keys, blob)
}

fn encrypt_bytes_with_keys(keys: &VaultKeys, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut src = data;
    let mut out = Vec::with_capacity(data.len() + HEADER_LEN + BLOCK + MAC_LEN + 8);
    encrypt_core(keys, &mut src, &mut out, data.len() as u64, &mut |_, _| {})?;
    Ok(out)
}

fn decrypt_bytes_with_keys(keys: &VaultKeys, blob: &[u8]) -> Result<Vec<u8>, String> {
    let mut src = blob;
    let mut out = Vec::new();
    decrypt_core(keys, &mut src, &mut out, &mut |_, _| {})?;
    Ok(out)
}
```

- [ ] **Step 6: 追加单测**

在文件末尾追加（与 `crypto.rs` 同款：因 `&AppHandle` 无法在单测中构造，故直接测可注入密钥的 `*_with_keys`）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> VaultKeys {
        VaultKeys {
            enc_key: [7u8; 32],
            mac_key: [9u8; 32],
        }
    }

    #[test]
    fn roundtrip_empty() {
        let k = keys();
        let blob = encrypt_bytes_with_keys(&k, b"").unwrap();
        assert!(blob.starts_with(MAGIC));
        assert_eq!(decrypt_bytes_with_keys(&k, &blob).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn roundtrip_small() {
        let k = keys();
        let data = b"hello-\xe4\xb8\xad\xe6\x96\x87".to_vec();
        let blob = encrypt_bytes_with_keys(&k, &data).unwrap();
        assert_ne!(blob, data);
        assert_eq!(decrypt_bytes_with_keys(&k, &blob).unwrap(), data);
    }

    #[test]
    fn roundtrip_multi_frame() {
        let k = keys();
        let data: Vec<u8> = (0..(CHUNK + 1234)).map(|i| (i % 251) as u8).collect();
        let blob = encrypt_bytes_with_keys(&k, &data).unwrap();
        assert_eq!(decrypt_bytes_with_keys(&k, &blob).unwrap(), data);
    }

    #[test]
    fn tampered_cipher_fails() {
        let k = keys();
        let mut blob = encrypt_bytes_with_keys(&k, b"secret payload").unwrap();
        blob[HEADER_LEN + 4 + IV_LEN] ^= 0x01;
        assert!(decrypt_bytes_with_keys(&k, &blob).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let a = keys();
        let b = VaultKeys {
            enc_key: [1u8; 32],
            mac_key: [2u8; 32],
        };
        let blob = encrypt_bytes_with_keys(&a, b"secret payload").unwrap();
        assert!(decrypt_bytes_with_keys(&b, &blob).is_err());
    }

    #[test]
    fn bad_magic_fails() {
        let k = keys();
        let mut blob = encrypt_bytes_with_keys(&k, b"x").unwrap();
        blob[0] = b'X';
        assert!(decrypt_bytes_with_keys(&k, &blob).is_err());
    }
}
```

- [ ] **Step 7: 验证**

Run: `cargo test`（在 `src-tauri/` 下）
Expected: 新增 6 个测试全绿；既有测试不受影响；无 `unused` 警告（`read_chunk`/`read_exact`/`init_mac` 均被核心函数使用）。

---

## Task 2: `commands.rs` 新增 `encrypt_upload_bytes` 与协议响应助手

**Files:**
- Modify: `src-tauri/src/commands.rs`

- [ ] **Step 1: 补导入**

把文件顶部导入区（`src-tauri/src/commands.rs:1-9`）

```rust
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;
```

改为

```rust
use std::path::{Path, PathBuf};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use tauri::{http, AppHandle, Emitter, Manager};
use uuid::Uuid;
```

- [ ] **Step 2: 新增命令 `encrypt_upload_bytes`**

在 `encrypt_upload_file` 之后插入。入参为内存 base64，产出与 `encrypt_upload_file` 完全一致的 `AttachmentMeta`（同一目录、同一 `.wst` 命名与原子写约定）：

```rust
/// 上传内存字节：base64 解码 → 内存加密 → 原子写入 `<upload_dir>/<uuid>.wst`
///
/// 供笔记编辑器粘贴 / 拖拽 / 选择图片使用，全程不产生明文临时文件。
#[tauri::command]
pub fn encrypt_upload_bytes(
    app: AppHandle,
    upload_dir: String,
    name: String,
    ext: String,
    data_b64: String,
) -> Result<AttachmentMeta, String> {
    let data = B64
        .decode(data_b64.as_bytes())
        .map_err(|e| format!("图片数据解码失败: {}", e))?;
    if data.is_empty() {
        return Err("图片内容为空".to_string());
    }
    let size = data.len() as u64;

    let dir = PathBuf::from(&upload_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("创建上传目录失败（{}）: {}", upload_dir, e))?;

    let blob = filecrypto::encrypt_bytes(&app, &data)?;

    let id = Uuid::new_v4().to_string();
    let tmp = dir.join(format!("{}.wst.tmp", id));
    let final_path = dir.join(format!("{}.wst", id));

    if let Err(e) = std::fs::write(&tmp, &blob) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入附件失败: {}", e));
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入附件失败: {}", e));
    }

    let ext = ext.trim().trim_start_matches('.').to_lowercase();
    let created_at = chrono::Utc::now().timestamp_millis();

    Ok(AttachmentMeta {
        id,
        name,
        size,
        ext,
        created_at,
        dir: upload_dir,
    })
}
```

- [ ] **Step 3: 新增协议响应助手与工具函数**

在 `remove_attachment_file` 之后追加。`serve_note_image` 供 `lib.rs` 的 `wst-img` 协议回调调用：

```rust
/// 解析笔记内联图片请求并返回响应：
/// `<prefix>/img?id=<uuid>&ext=<png>&dir=<urlencoded 绝对目录>`
///
/// 仅服务本应用密文（`.wst`）；`id` 必须为 uuid 形态，防目录穿越。
pub fn serve_note_image(app: &AppHandle, uri: &http::Uri) -> http::Response<Vec<u8>> {
    let params = parse_query(uri.query().unwrap_or(""));
    let id = params.get("id").cloned().unwrap_or_default();
    let ext = params.get("ext").cloned().unwrap_or_default();
    let dir = params.get("dir").cloned().unwrap_or_default();

    if id.is_empty() || dir.is_empty() || Uuid::parse_str(&id).is_err() {
        return plain_response(400, "请求参数不合法");
    }

    let blob_path = PathBuf::from(&dir).join(format!("{}.wst", id));
    let blob = match std::fs::read(&blob_path) {
        Ok(bytes) => bytes,
        Err(_) => return plain_response(404, "图片数据缺失"),
    };

    match filecrypto::decrypt_bytes(app, &blob) {
        Ok(plain) => http::Response::builder()
            .status(200)
            .header(http::header::CONTENT_TYPE, content_type_for_ext(&ext))
            .header(http::header::CACHE_CONTROL, "no-store")
            .body(plain)
            .unwrap_or_else(|_| plain_response(500, "构建响应失败")),
        Err(_) => plain_response(500, "图片数据损坏"),
    }
}

/// 构造一个 `text/plain` 的简单响应
fn plain_response(status: u16, message: &str) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| http::Response::new(Vec::new()))
}

/// 扩展名 → `Content-Type`
fn content_type_for_ext(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    }
}

/// 解析 `a=1&b=2` 形式的查询串（含百分号解码）
fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}

/// 最小百分号解码（`%XX` 与 `+`），避免为解析 URL 引入新依赖
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push((hi << 4) | lo);
                    i += 3;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
```

- [ ] **Step 4: 验证编译**

Run: `cargo check`（在 `src-tauri/` 下）
Expected: 通过。`serve_note_image` 此时尚无调用方，可能报 `dead_code` 警告——由 Task 3 接线后消除；若 `cargo check` 因 `-D warnings` 失败，先完成 Task 3 再复检。

---

## Task 3: `lib.rs` 注册 `wst-img` 协议并挂载命令

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: `invoke_handler` 挂载新命令**

在 `commands::remove_attachment_file,`（`src-tauri/src/lib.rs:135`）之后插入一行：

```rust
            commands::encrypt_upload_bytes,
```

- [ ] **Step 2: 注册自定义 URI 协议**

在 `run()` 的构建链上、`.plugin(tauri_plugin_process::init())`（`src-tauri/src/lib.rs:104`）之后插入。回调只做「取 app handle → 交给 `serve_note_image` → 回包」，不阻塞主线程：

```rust
        .register_asynchronous_uri_scheme_protocol("wst-img", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let response = commands::serve_note_image(&app, request.uri());
            responder.respond(response);
        })
```

- [ ] **Step 3: 验证编译**

Run: `cargo check`（在 `src-tauri/` 下）
Expected: 通过，`dead_code` 警告消失；`register_asynchronous_uri_scheme_protocol` 的闭包签名与 `serve_note_image` 的 `&http::Uri` 入参匹配。

---

## Task 4: 引入 `md-editor-v3` 依赖

**Files:**
- Modify: `package.json`（由包管理器写入）、`pnpm-lock.yaml`

- [ ] **Step 1: 安装依赖**

Run: `pnpm add md-editor-v3@^7.1.0`（仓库根目录）
Expected: `package.json` 的 `dependencies` 出现 `"md-editor-v3": "^7.1.0"`；`node_modules/md-editor-v3` 存在；`md-editor-v3/lib/style.css` 与 `md-editor-v3/lib/types/index.d.ts` 可访问。

- [ ] **Step 2: 确认版本与导出**

Run: `node -e "const p=require('./node_modules/md-editor-v3/package.json');console.log(p.version)"`
Expected: 打印 `7.x`（≥ 7.1.0）。若为 6.x/8.x，改用该版本的类型定义核对工具栏 key 白名单后再继续 Task 5。

---

## Task 5: 重写 `NoteEditor.vue`

**Files:**
- Modify: `src/components/NoteEditor.vue`（整文件替换）

- [ ] **Step 1: 替换 `<script setup>` 与模板**

用下列内容整体替换该文件（保留原「500ms 防抖 + 卸载补交」语义）：

```vue
<script setup lang="ts">
import { ref, watch, onBeforeUnmount } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { MdEditor } from 'md-editor-v3'
import type { ToolbarNames } from 'md-editor-v3'
import 'md-editor-v3/lib/style.css'
import { isWindows } from '@/platform'
import type { AttachmentMeta, LogLevel } from '@/types'

const props = defineProps<{
  note: string
  uploadDir: string | null
  defaultUploadDir: string | null
}>()

const emit = defineEmits<{
  update: [note: string]
  addAttachment: [meta: AttachmentMeta]
  log: [level: LogLevel, message: string]
}>()

const text = ref(props.note ?? '')

// 外部（切换实体）传入新笔记时同步到本地
watch(
  () => props.note,
  (value) => {
    const next = value ?? ''
    if (next !== text.value) text.value = next
  },
)

// 本地编辑防抖回传，沿用 ProjectConfig 的保存模式
let timer: number | undefined
watch(text, (value) => {
  if (timer) window.clearTimeout(timer)
  timer = window.setTimeout(() => {
    timer = undefined
    emit('update', value)
  }, 500)
})

onBeforeUnmount(() => {
  if (timer) {
    window.clearTimeout(timer)
    timer = undefined
  }
  // 卸载前把未落盘的编辑补交，避免快速切换实体丢内容
  if (text.value !== (props.note ?? '')) emit('update', text.value)
})

const toolbars: ToolbarNames[] = [
  'bold',
  'italic',
  'strikeThrough',
  'title',
  'quote',
  'unorderedList',
  'orderedList',
  'task',
  'codeRow',
  'code',
  'link',
  'image',
  'table',
  '-',
  'revoke',
  'next',
  '-',
  'prettier',
  'catalog',
  'preview',
]

// 自定义协议前缀：macOS/Linux 为 wst-img://，Windows 为 http://wst-img.localhost
function imageUrl(id: string, ext: string, dir: string): string {
  const prefix = isWindows() ? 'http://wst-img.localhost' : 'wst-img://localhost'
  const query = `id=${encodeURIComponent(id)}&ext=${encodeURIComponent(ext)}&dir=${encodeURIComponent(dir)}`
  return `${prefix}/img?${query}`
}

function readFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      const result = reader.result
      if (typeof result !== 'string') {
        reject(new Error('读取文件失败'))
        return
      }
      const idx = result.indexOf(',')
      resolve(idx >= 0 ? result.slice(idx + 1) : result)
    }
    reader.onerror = () => reject(reader.error ?? new Error('读取文件失败'))
    reader.readAsDataURL(file)
  })
}

function extOf(file: File): string {
  const name = file.name || ''
  const idx = name.lastIndexOf('.')
  if (idx >= 0 && idx < name.length - 1) return name.slice(idx + 1).toLowerCase()
  const type = file.type || ''
  const slash = type.indexOf('/')
  return slash >= 0 ? type.slice(slash + 1).toLowerCase() : ''
}

async function onUploadImg(files: File[], callback: (urls: string[]) => void) {
  let uploadDir: string
  try {
    uploadDir = await invoke<string>('resolve_upload_dir', {
      entityDir: props.uploadDir,
      defaultDir: props.defaultUploadDir,
    })
  } catch (e) {
    emit('log', 'error', `解析上传目录失败: ${e instanceof Error ? e.message : String(e)}`)
    callback([])
    return
  }

  const urls: string[] = []
  for (const file of files) {
    try {
      const dataB64 = await readFileAsBase64(file)
      if (!dataB64) throw new Error('文件内容为空')
      const meta = await invoke<AttachmentMeta>('encrypt_upload_bytes', {
        uploadDir,
        name: file.name || 'image',
        ext: extOf(file),
        dataB64,
      })
      emit('addAttachment', meta)
      emit('log', 'success', `图片已加密上传: ${meta.name}`)
      urls.push(imageUrl(meta.id, meta.ext, meta.dir))
    } catch (e) {
      emit('log', 'error', `图片上传失败: ${file.name} - ${e instanceof Error ? e.message : String(e)}`)
    }
  }
  callback(urls)
}
</script>

<template>
  <div class="note-editor">
    <MdEditor
      v-model="text"
      class="note-md-editor"
      language="zh-CN"
      theme="light"
      preview-theme="github"
      code-theme="github"
      :toolbars="toolbars"
      :show-code-row-number="true"
      :no-mermaid="true"
      :no-katex="true"
      :on-upload-img="onUploadImg"
    />
  </div>
</template>

<style scoped>
.note-editor {
  height: 100%;
  min-height: 0;
}
.note-md-editor {
  height: 100%;
}
.note-editor :deep(.md-editor) {
  height: 100%;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  --md-bk-color: var(--bg-primary);
  --md-color: var(--text-primary);
  --md-border-color: var(--border-color);
}
.note-editor :deep(.md-editor-toolbar-wrapper) {
  border-bottom: 1px solid var(--border-color);
}
</style>
```

- [ ] **Step 2: 验证类型与构建**

Run: `pnpm exec vue-tsc --noEmit`（仓库根目录）
Expected: 通过，无类型错误。重点核对：`ToolbarNames[]` 中每个字符串都属于 §「关键前置」列出的 7.1.0 白名单；`:on-upload-img` 绑定的是 prop 回调。

- [ ] **Step 3: 目视确认（Task 8 实机时）**

在 Task 8 重启后确认：笔记页签呈现「工具条 + 源码 + 右侧预览」，`catalog` 可打开目录大纲，代码块有高亮，表格 / 任务列表可渲染，编辑器撑满页签。若不撑满或边框异常，按实际渲染类名微调 Step 1 的 `<style>`（类名/变量见「待实现时确认项」第 2 条）。

---

## Task 6: 连线 `ProjectDetail.vue` / `GroupDetail.vue`

**Files:**
- Modify: `src/components/ProjectDetail.vue:63`
- Modify: `src/components/GroupDetail.vue:30`

- [ ] **Step 1: `ProjectDetail.vue` 的 `<NoteEditor>`**

把

```html
          <NoteEditor :note="project.note" @update="(note) => emit('updateNote', note)" />
```

改为

```html
          <NoteEditor
            :note="project.note"
            :upload-dir="project.upload_dir"
            :default-upload-dir="defaultUploadDir"
            @update="(note) => emit('updateNote', note)"
            @add-attachment="(meta) => emit('addAttachment', meta)"
            @log="(level, message) => emit('log', level, message)"
          />
```

- [ ] **Step 2: `GroupDetail.vue` 的 `<NoteEditor>`**

把

```html
        <NoteEditor :note="group.note" @update="(note) => emit('updateNote', note)" />
```

改为

```html
        <NoteEditor
          :note="group.note"
          :upload-dir="group.upload_dir"
          :default-upload-dir="defaultUploadDir"
          @update="(note) => emit('updateNote', note)"
          @add-attachment="(meta) => emit('addAttachment', meta)"
          @log="(level, message) => emit('log', level, message)"
        />
```

- [ ] **Step 3: 验证**

Run: `pnpm exec vue-tsc --noEmit`
Expected: 通过。两组件均已 `import type { AttachmentMeta, LogLevel, ... }`，`emit('addAttachment')` / `emit('log')` 已在各自 `defineEmits` 中声明，转发无需新增类型。

---

## Task 7: 移除 `marked` / `dompurify`

**Files:**
- Modify: `package.json`（由包管理器写入）、`pnpm-lock.yaml`

- [ ] **Step 1: 确认无残留引用**

Run: `grep -rn "marked\|dompurify\|DOMPurify" src`（仓库根目录）
Expected: 无输出（重写后的 `NoteEditor.vue` 已不再引用；其他文件本就没有）。

- [ ] **Step 2: 移除依赖**

Run: `pnpm remove marked dompurify`（仓库根目录）
Expected: `package.json` 的 `dependencies` 不再含 `marked`、`dompurify`；`node_modules` 中对应目录被清理。

- [ ] **Step 3: 验证**

Run: `pnpm build`
Expected: `vue-tsc --noEmit && vite build` 均通过。

---

## Task 8: 整体构建与重启验证

**Files:** 无（仅运行）

- [ ] **Step 1: 后端测试与编译**

Run: `cargo test` 然后 `cargo check`（均在 `src-tauri/` 下）
Expected: 全部通过（含 Task 1 新增 6 个测试）。

- [ ] **Step 2: 前端构建**

Run: `pnpm build`（仓库根目录）
Expected: 通过。

- [ ] **Step 3: 按项目规则重启应用**

macOS 先结束残留进程再拉起（**勿用** `pnpm tauri dev`）：

```bash
pkill -f "ws-workbench" 2>/dev/null; pkill -f "tauri dev" 2>/dev/null; pkill -f "vite" 2>/dev/null
pnpm exec tauri dev
```

- [ ] **Step 4: 手工验收（对照设计文档 §10，本机 macOS）**

1. 笔记页签呈现「工具栏 + 源码 + 右侧实时预览」；`catalog` 打开目录大纲；代码块有高亮；表格与任务列表可用。
2. 粘贴 / 拖拽 / 选择一张图片 → 目标目录出现新 `<uuid>.wst`（用文本工具/`head -c` 查看为二进制密文，非图片明文）；预览内联显示该图；上传页签同步出现该附件。
3. 重启应用后：笔记文字与内联图片仍在、可正常显示（验证协议 + 密文持久化）。
4. 在上传页签下载该附件，字节与原始图片逐字节一致（`shasum` 对比），文件名可还原。
5. 中文界面、浅色主题与现有设计令牌一致，编辑器撑满页签。
6. 边界：删除该附件后笔记图片变裂图（可接受）；对只读目录上传时日志报「创建上传目录失败（路径）」且不留 `.wst.tmp`。

---

## Self-Review

### 1. Spec 覆盖对照

| 设计文档章节 | 覆盖任务 | 状态 |
| --- | --- | --- |
| §3 选型与依赖（`md-editor-v3`、配置项、移除 `marked`/`dompurify`） | Task 4、Task 5、Task 7 | 已覆盖 |
| §4.1 `filecrypto` 抽核心 + `encrypt_bytes`/`decrypt_bytes` | Task 1 | 已覆盖 |
| §4.2 命令 `encrypt_upload_bytes` | Task 2 | 已覆盖 |
| §4.3 注册 `wst-img` 协议（参数、uuid 校验、ext→Content-Type、404/500、跨平台前缀） | Task 3、Task 2、Task 5 | 已覆盖 |
| §5.1 `NoteEditor.vue` 重写（props/emits、工具栏、防抖、`onUploadImg` 流程、目录/高亮/表格） | Task 5 | 已覆盖 |
| §5.2 父组件连线（两 `*Detail.vue`；`App.vue` 不动） | Task 6 | 已覆盖 |
| §6 图片引用格式（`wst-img://...` + uuid/ext/dir） | Task 5（`imageUrl`） | 已覆盖 |
| §7 与现有附件体系关系（同一批 `AttachmentMeta`/`.wst`，push 即落盘） | Task 2、Task 6 | 已覆盖 |
| §8 异常与边界（单图失败跳过、目录不可写、base64 空、blob/HMAC 失败、原子写） | Task 2、Task 5、Task 1 | 已覆盖 |
| §9 改动文件清单（7 个文件） | Task 1–7 | 已覆盖 |
| §10 验收标准 1–6 | Task 5、Task 8 | 已覆盖 |
| §11 待实现时确认项 | 「待实现时确认项」小节 + Self-Review §4 | 已记录 |

### 2. Placeholder 扫描

- 全文无 `TODO` / `FIXME` / `...` 省略占位；每个待办步骤均给出可直接落地的完整代码。
- Task 5 的 `<style>` 中类名/变量为 md-editor-v3 v3 公开类名，若实际不同按「待实现时确认项」第 2 条在 Task 8 微调——这是**实机视觉校准**，非代码占位。
- `serve_note_image` 的文档注释即规格，无未实现分支。

### 3. 类型与命名一致性核对

- Rust `AttachmentMeta { id, name, size: u64, ext, created_at: i64, dir }` ↔ TS `AttachmentMeta { id, name, size: number, ext, created_at: number, dir }`，字段一一对应；`encrypt_upload_bytes` 返回该结构，Task 5 以 `invoke<AttachmentMeta>` 接收。
- 命令参数：Rust `upload_dir`/`data_b64` ↔ 前端 `invoke` 用 camelCase `uploadDir`/`dataB64`（Tauri 自动转换）；`resolve_upload_dir` 的 `entity_dir`/`default_dir` ↔ `entityDir`/`defaultDir`，与既有调用一致。
- 协议入参命名统一为 `id`/`ext`/`dir`，前端 `imageUrl()` 与后端 `serve_note_image` 严格对齐。
- 组件 props/emits：子组件 `defineEmits` 用 camelCase（`addAttachment`、`log`、`update`），模板监听用 kebab-case（`@add-attachment`、`@log`、`@update`）；`ProjectDetail`/`GroupDetail` 转发到既有 `addAttachment`/`log` emit，`App.vue` 监听名不变。
- `LogLevel` 取值含 `'success'`（`src/types/index.ts`），Task 5 的 `emit('log','success',...)` 合法。
- 平台判定：前端 `isWindows()`（`src/platform.ts:80`）与后端 `PlatformInfo.is_windows` 对齐，仅前端用于选协议前缀。

### 4. 已知取舍

- **离线代码高亮**：md-editor-v3 默认从 `https://unpkg.com` 加载 highlight.js / prettier / mermaid / katex / 代码主题 CSS；本计划仅按设计开了 `noMermaid` / `noKatex`（停用两者，减少 CDN 依赖与体积），**未**默认接管 highlight.js。若验收环境离线导致高亮或 `codeTheme="github"` 失效，按「待实现时确认项」第 1 条补 `pnpm add highlight.js` + `config({ editorExtensions: { highlight: { instance: hljs } } })`。此为设计 §3 未纳入的额外依赖，故不在本计划强制项内。
- **图片 URL 含本机绝对路径**：笔记导出/换机后图片失效；删除附件后笔记图片变裂图。均与设计 §6 一致，本地单机场景可接受。
- **内存 base64 传输**：截图量级（数 MB）无压力；超大图仍建议走上传页签（设计 §8 已标注）。
- **协议鉴权**：仅服务 `.wst`、`id` 做 uuid 白名单校验；单用户本地桌面应用，未加额外 token，风险可接受（设计 §4.3）。
- **`wst-img` 回包无 CSP 头**：因 `tauri.conf.json` `csp: null`，无需处理；若未来收紧 CSP，需把 `wst-img:` / `http://wst-img.localhost` 加入 `img-src`。
- **不重启不改动生效**：按项目规则，每个任务后需 `pnpm exec tauri dev` 重启验证。

---

## Execution Handoff

计划已保存至 `docs/superpowers/plans/2026-09-30-note-editor-md-editor-v3.md`。请选择执行方式：

1. **Subagent-Driven（推荐）** —— 用 superpowers:subagent-driven-development，每个任务（Task 1–8）派独立子代理实现、任务间停下复核；适合本特性「后端 3 个文件 → 前端 3 个文件 → 联调」的层次化推进。
2. **Inline Execution** —— 用 superpowers:executing-plans，在当前会话按 Task 1→8 顺序批量执行并设检查点。

请回复所选方式，我将据此开始实现。
