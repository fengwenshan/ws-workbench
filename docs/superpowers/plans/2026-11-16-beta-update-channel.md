# 内测（Beta）更新通道实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 ws-workbench 加「正式 / 内测」双更新通道：内测版本号 `<base>-beta.<N>`，Gitee 上单独发 beta 清单，客户端设置里一个开关切换，关闭不降级、下个正式版自然收敛。

**Architecture:** Rust 新增 `update_check(channel)` 命令，按通道选清单、取版本更高者（稳定清单不动，beta 清单只在 Gitee）；前端用这个命令替换插件 `check()`，把通道持久化到 `config.json`；发布侧 beta 走本地 `release.mjs --beta` → Gitee，正式版改由 push `main` 触发 GitHub Actions（preflight 按 `tauri.conf.json` 版本号判断是否需要发版）。

**Tech Stack:** Rust / Tauri 2 / tauri-plugin-updater 2.11 / semver / Vue 3 + TypeScript / Node.js 22 发布脚本 / GitHub Actions。

**Spec:** `docs/superpowers/specs/2026-11-16-beta-update-channel-design.md`

---

## 前置约定（先做，再进任务）

所有实现提交都在 **`beta` 分支**上做；正式版由「合并到 main」触发。

- [ ] **Step 0.1：切到 beta 分支并开新 beta 线**

```bash
git fetch origin
git switch beta 2>/dev/null || git switch -c beta origin/main
```

- [ ] **Step 0.2：把 beta 线的 base 版本提到下一个正式号**

改 `src-tauri/tauri.conf.json` 顶部的 `"version": "1.0.0"` → `"version": "1.0.25"`
（按实际最新正式版取 +1；当前线上是 1.0.24）。

```bash
git add src-tauri/tauri.conf.json
git commit -m "chore(release): beta 线 base 版本提到 1.0.25"
```

> 注意：**不要**在这个提交里改 `package.json` / `Cargo.toml` 的版本。Tauri 的版本号只认
> `tauri.conf.json`，构建时脚本会再用 `--config` 注入 beta 后缀。

- [ ] **Step 0.3：确认验证命令能用**

```bash
cd src-tauri && cargo check --quiet
cd .. && pnpm build
```

Expected：两条都退出码 0（`pnpm build` 会跑 `vue-tsc --noEmit && vite build`，第一次可能较慢）。

---

### Task 1: Rust —— beta 端点派生与版本比较（纯函数 + 单测）

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/updates.rs`（在 `#[cfg(test)] mod tests` 之前加辅助函数，测试加在 `mod tests` 末尾）

- [ ] **Step 1: 先写会失败的测试**

在 `src-tauri/src/updates.rs` 的 `mod tests` 末尾（`parses_content_range` 之后、`}` 之前）加：

```rust
    #[test]
    fn derives_beta_endpoints_from_gitee_only() {
        let stable: Vec<Url> = [
            "https://gitee.com/feng_wenshan/ws-workbench/raw/master/updates/latest.json",
            "https://gitee.com/feng_wenshan/ws-workbench/raw/main/updates/latest.json",
            "https://raw.githubusercontent.com/fengwenshan/ws-workbench/main/updates/latest.json",
        ]
        .iter()
        .map(|url| Url::parse(url).expect("测试端点"))
        .collect();

        let beta: Vec<String> = derive_beta_endpoints(&stable)
            .iter()
            .map(|url| url.to_string())
            .collect();

        assert_eq!(
            beta,
            vec![
                "https://gitee.com/feng_wenshan/ws-workbench/raw/master/updates/beta/latest.json"
                    .to_string(),
                "https://gitee.com/feng_wenshan/ws-workbench/raw/main/updates/beta/latest.json"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn version_comparison_handles_prerelease() {
        assert_eq!(version_is_newer("1.0.18", "1.0.18-beta.14"), Some(true));
        assert_eq!(version_is_newer("1.0.18-beta.14", "1.0.18"), Some(false));
        assert_eq!(
            version_is_newer("1.0.18-beta.14", "1.0.18-beta.2"),
            Some(true)
        );
        assert_eq!(
            version_is_newer("1.0.18-beta.2", "1.0.18-beta.14"),
            Some(false)
        );
        assert_eq!(version_is_newer("1.0.18", "1.0.18"), Some(false));
        assert_eq!(version_is_newer("not-a-version", "1.0.18"), None);
    }

    #[test]
    fn pick_channel_prefers_stable_on_tie_or_parse_error() {
        assert_eq!(pick_channel("1.0.18-beta.14", "1.0.18"), "stable");
        assert_eq!(pick_channel("1.0.19-beta.1", "1.0.18"), "beta");
        assert_eq!(pick_channel("bad", "1.0.18"), "stable");
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cd src-tauri && cargo test --quiet version_comparison_handles_prerelease 2>&1 | tail -20
```

Expected：编译失败，报 `cannot find function version_is_newer` / `derive_beta_endpoints` / `pick_channel`。

- [ ] **Step 3: 加依赖**

`src-tauri/Cargo.toml` 的 `[dependencies]` 里、`serde_json = "1"` 之后加：

```toml
# 通道检查要比较两个清单的版本号：插件的 Update.version 是字符串，比较逻辑在它内部，
# 这里显式依赖 semver 自己做「beta 清单 vs 稳定清单取高」。
semver = "1"
```

- [ ] **Step 4: 实现辅助函数**

`src-tauri/src/updates.rs` 顶部把

```rust
use tauri_plugin_updater::Update;
```

改成：

```rust
use tauri_plugin_updater::{Update, UpdaterExt};
```

在 `#[cfg(test)] mod tests {` 之前（也就是 `verify_signature` 等函数之后）加：

```rust
// ========== 更新通道（stable / beta） ==========

/// 稳定清单在端点 URL 里的固定尾巴，beta 清单由它替换得到。
const STABLE_MANIFEST_SUFFIX: &str = "/updates/latest.json";
const BETA_MANIFEST_SUFFIX: &str = "/updates/beta/latest.json";

/// 从配置的稳定端点派生出 beta 清单端点。
///
/// beta 安装包与清单只在 Gitee 发布（GitHub 只放正式版），所以这里只认 gitee.com；
/// 以后要加别的镜像，把主机名补进这个过滤条件即可。
fn derive_beta_endpoints(stable: &[Url]) -> Vec<Url> {
    stable
        .iter()
        .filter(|url| url.host_str().map(|host| host == "gitee.com").unwrap_or(false))
        .filter_map(|url| {
            let text = url.as_str();
            let prefix = text.strip_suffix(STABLE_MANIFEST_SUFFIX)?;
            Url::parse(&format!("{prefix}{BETA_MANIFEST_SUFFIX}")).ok()
        })
        .collect()
}

/// 版本号比较（semver，支持 `-beta.N` 预发布）：`a > b` 返回 `Some(true)`。
/// 解析失败返回 `None`，调用方按「不取 beta」处理。
fn version_is_newer(a: &str, b: &str) -> Option<bool> {
    let a = semver::Version::parse(a).ok()?;
    let b = semver::Version::parse(b).ok()?;
    Some(a > b)
}

/// 两个通道都查到更新时选哪个：beta 必须严格更高才用 beta，否则用稳定版。
fn pick_channel(beta: &str, stable: &str) -> &'static str {
    match version_is_newer(beta, stable) {
        Some(true) => "beta",
        _ => "stable",
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

```bash
cd src-tauri && cargo test --quiet version_comparison_handles_prerelease derives_beta_endpoints pick_channel 2>&1 | tail -20
```

Expected：三个测试 PASS（`cargo test` 会顺带跑全量，其它已有测试也应保持通过）。

- [ ] **Step 6: 提交**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/updates.rs
git commit -m "feat(update): beta 清单端点派生与版本比较"
```

---

### Task 2: Rust —— `update_check(channel)` 命令

**Files:**
- Modify: `src-tauri/src/updates.rs`（命令与元数据）
- Modify: `src-tauri/src/lib.rs`（注册命令）

- [ ] **Step 1: 先写会失败的测试**

在 `mod tests` 里加（`choose_channel` 把命令里的「选哪个清单」逻辑抽成纯函数，命令本身依赖 Webview 没法直接测）：

```rust
    #[test]
    fn choose_channel_covers_fallback_cases() {
        // beta 清单 404 / 没有本平台条目：退回稳定清单
        assert_eq!(choose_channel(None, Some("1.0.18")), Some("stable"));
        // 两个清单都有：beta 严格更高才用 beta
        assert_eq!(
            choose_channel(Some("1.0.19-beta.1"), Some("1.0.18")),
            Some("beta")
        );
        // 正式版追上 beta 线：beta 用户升到正式版（1.0.18 > 1.0.18-beta.14）
        assert_eq!(
            choose_channel(Some("1.0.18-beta.14"), Some("1.0.18")),
            Some("stable")
        );
        // 两个都没有：没有更新可装
        assert_eq!(choose_channel(None, None), None);
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cd src-tauri && cargo test --quiet choose_channel_covers_fallback_cases 2>&1 | tail -10
```

Expected：编译失败，报 `cannot find function choose_channel`。

- [ ] **Step 3: 实现命令**

在 `src-tauri/src/updates.rs` 的 `take_update` 之后加：

```rust
/// 选通道：`None` 表示该通道「没有可用更新」（beta 清单 404 / 缺本平台条目 / 解析失败）。
/// 两个都有时按 semver 取高，相等或解析失败取稳定版。
fn choose_channel(beta: Option<&str>, stable: Option<&str>) -> Option<&'static str> {
    match (beta, stable) {
        (Some(beta), Some(stable)) => Some(pick_channel(beta, stable)),
        (Some(_), None) => Some("beta"),
        (None, Some(_)) => Some("stable"),
        (None, None) => None,
    }
}

/// `update_check` 的返回元数据。字段名与前端 `src/api/updater.ts` 约定一致，勿改。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMetadata {
    rid: ResourceId,
    current_version: String,
    version: String,
    date: Option<String>,
    body: Option<String>,
    raw_json: serde_json::Value,
    /// 这次结果来自哪个通道：`stable` / `beta`（UI 用来提示）
    channel: String,
}

/// 读取 `tauri.conf.json` 里配置的更新端点（和插件用的是同一份配置）。
fn configured_endpoints(app: &AppHandle) -> Vec<Url> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("endpoints"))
        .and_then(|endpoints| endpoints.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .filter_map(|text| Url::parse(text).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// 用给定端点（`None` = 用配置里的默认端点）检查一次更新。
async fn check_with_endpoints(
    webview: &Webview,
    endpoints: Option<Vec<Url>>,
    timeout: Duration,
) -> Result<Option<Update>, String> {
    let mut builder = webview.updater_builder().timeout(timeout);
    if let Some(endpoints) = endpoints {
        if endpoints.is_empty() {
            return Ok(None);
        }
        builder = builder
            .endpoints(endpoints)
            .map_err(|err| format!("更新端点无效：{err}"))?;
    }
    builder
        .build()
        .map_err(|err| format!("更新检查初始化失败：{err}"))?
        .check()
        .await
        .map_err(|err| err.to_string())
}

/// 检查更新：`stable` 只看稳定清单；`beta` 同时看 beta 清单和稳定清单，取版本更高者。
///
/// `Update` 放进 webview 的 resource table（与插件自带 `check` 命令一致），
/// 后续 `update_download_resumed` / `update_install_resumed` 靠 rid 取回。
#[tauri::command]
pub async fn update_check(
    webview: Webview,
    channel: String,
    timeout: Option<u64>,
) -> Result<Option<UpdateMetadata>, String> {
    let timeout = Duration::from_millis(timeout.unwrap_or(10_000));
    let endpoints = configured_endpoints(&webview.app_handle());

    // 稳定清单：网络/解析错误先记下来，只有 beta 也查不到时才把错误抛给界面
    let mut stable_error: Option<String> = None;
    let stable_update = match check_with_endpoints(&webview, None, timeout).await {
        Ok(update) => update,
        Err(err) => {
            stable_error = Some(err);
            None
        }
    };

    let beta_update = if channel == "beta" {
        let beta_endpoints = derive_beta_endpoints(&endpoints);
        // beta 清单缺失 / 没有本平台条目 / 超时，一律当「没有 beta 更新」，不打断稳定检查
        check_with_endpoints(&webview, Some(beta_endpoints), timeout)
            .await
            .unwrap_or(None)
    } else {
        None
    };

    let chosen = choose_channel(
        beta_update.as_ref().map(|update| update.version.as_str()),
        stable_update.as_ref().map(|update| update.version.as_str()),
    );
    let update = match chosen {
        Some("beta") => beta_update.expect("choose_channel 选了 beta 却没有 beta_update"),
        Some(_) => stable_update.expect("choose_channel 选了 stable 却没有 stable_update"),
        None => {
            return match stable_error {
                Some(err) => Err(err),
                // beta 通道里还没有构建过任何包时，这属于正常状态
                None => Ok(None),
            };
        }
    };
    let chosen_channel = chosen.expect("上面已经处理 None");

    // 清单里的 pub_date 就是插件会格式化出来的同一个值，直接取字符串，避免再引 time crate
    let date = update
        .raw_json
        .get("pub_date")
        .and_then(|value| value.as_str())
        .map(str::to_string);

    let metadata = UpdateMetadata {
        current_version: update.current_version.clone(),
        version: update.version.clone(),
        date,
        body: update.body.clone(),
        raw_json: update.raw_json.clone(),
        channel: chosen_channel.to_string(),
        rid: webview.resources_table().add(update),
    };
    Ok(Some(metadata))
}
```

- [ ] **Step 4: 注册命令**

`src-tauri/src/lib.rs` 里找到：

```rust
            // 更新包断点续传：下载 / 安装（前端 useUpdater 调用）
            updates::update_download_resumed,
            updates::update_install_resumed,
```

改成：

```rust
            // 更新包断点续传：下载 / 安装（前端 useUpdater 调用）
            // 更新检查：按通道（stable / beta）选清单，取代前端直接调插件 check()
            updates::update_check,
            updates::update_download_resumed,
            updates::update_install_resumed,
```

- [ ] **Step 5: 编译并跑测试**

```bash
cd src-tauri && cargo check --quiet && cargo test --quiet 2>&1 | tail -20
```

Expected：`cargo check` 0；全量测试 PASS。

- [ ] **Step 6: 提交**

```bash
git add src-tauri/src/updates.rs src-tauri/src/lib.rs
git commit -m "feat(update): update_check 按通道检查更新"
```

---

### Task 3: 配置字段 `update_channel`（Rust + TS）

**Files:**
- Modify: `src-tauri/src/models.rs`（`AppSettings`、`AppConfig`）
- Modify: `src-tauri/src/config.rs`（`from_config` / `into_config`）
- Modify: `src-tauri/src/config.rs`（测试）
- Modify: `src/types/index.ts`
- Modify: `src/composables/useConfig.ts`

- [ ] **Step 1: 先写会失败的测试**

`src-tauri/src/config.rs` 的 `mod tests` 里加：

```rust
    #[test]
    fn update_channel_defaults_to_empty() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.update_channel, "");
    }
```

并把已有的 `app_settings_roundtrip_from_config` 测试改成（新增字段与断言）：

```rust
    #[test]
    fn app_settings_roundtrip_from_config() {
        let config = AppConfig {
            default_include: vec!["*.js".to_string()],
            default_include_version: 7,
            vpn_active_profile_id: "p1".to_string(),
            vpn_worker_session_dir: "/tmp/session".to_string(),
            default_upload_dir: Some("/tmp/upload".to_string()),
            tree_order: vec!["g1".to_string(), "r1".to_string()],
            update_channel: "beta".to_string(),
            ..Default::default()
        };
        let settings = AppSettings::from_config(&config);
        assert_eq!(settings.update_channel, "beta");
        let merged = settings.into_config(crate::db::DbData::default());
        assert_eq!(merged.update_channel, "beta");
        assert_eq!(merged.default_include, config.default_include);
        assert_eq!(merged.default_include_version, 7);
        assert_eq!(merged.vpn_active_profile_id, "p1");
        assert_eq!(merged.vpn_worker_session_dir, "/tmp/session");
        assert_eq!(merged.default_upload_dir, config.default_upload_dir);
        assert_eq!(merged.tree_order, config.tree_order);
        assert!(merged.groups.is_empty());
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cd src-tauri && cargo test --quiet update_channel_defaults_to_empty 2>&1 | tail -10
```

Expected：编译失败，报 `no field update_channel`。

- [ ] **Step 3: 加 Rust 字段与映射**

`src-tauri/src/models.rs` 的 `AppSettings` 结构体里（`tree_order` 之后）加：

```rust
    /// 更新通道：空串 = 跟随安装版本（预发布版按 beta）；"stable" / "beta" 是用户显式选择
    #[serde(default)]
    pub update_channel: String,
```

`AppSettings::default()` 里加：

```rust
            update_channel: String::new(),
```

`AppConfig` 结构体里（`tree_order` 之后）加：

```rust
    /// 更新通道，见 [`AppSettings::update_channel`]
    #[serde(default)]
    pub update_channel: String,
```

`AppConfig::default()` 里加：

```rust
            update_channel: String::new(),
```

`src-tauri/src/config.rs` 的 `AppSettings::from_config` 里加：

```rust
            update_channel: config.update_channel.clone(),
```

`into_config` 里加：

```rust
            update_channel: self.update_channel,
```

- [ ] **Step 4: 跑测试确认通过**

```bash
cd src-tauri && cargo test --quiet app_settings_roundtrip_from_config update_channel_defaults_to_empty 2>&1 | tail -10
```

Expected：PASS。

- [ ] **Step 5: 前端类型**

`src/types/index.ts` 的 `AppConfig` 里加：

```ts
  /** 更新通道：'' / 缺省 = 跟随安装版本（预发布版按 beta）；'stable' / 'beta' 为用户显式选择 */
  update_channel?: 'stable' | 'beta' | ''
```

`src/composables/useConfig.ts` 的 `loadConfig` 里，把

```ts
        default_upload_dir: loaded.default_upload_dir ?? null,
```

改成：

```ts
        default_upload_dir: loaded.default_upload_dir ?? null,
        update_channel: loaded.update_channel ?? '',
```

- [ ] **Step 6: 前端类型检查**

```bash
pnpm build 2>&1 | tail -10
```

Expected：退出码 0。

- [ ] **Step 7: 提交**

```bash
git add src-tauri/src/models.rs src-tauri/src/config.rs src/types/index.ts src/composables/useConfig.ts
git commit -m "feat(config): 持久化更新通道 update_channel"
```

---

### Task 4: 前端 —— 按通道检查更新（API + useUpdater）

**Files:**
- Create: `src/api/updater.ts`
- Modify: `src/api/app.ts`（新增 `isPrereleaseBuild`）
- Modify: `src/composables/useUpdater.ts`（通道状态、改用后端命令、`setChannel`）

- [ ] **Step 1: 新增 `src/api/updater.ts`**

```ts
//! 更新检查：按通道（stable / beta）走后端的 `update_check` 命令。
//!
//! 选端点、两个清单取版本更高者都在 Rust 侧做（见 src-tauri/src/updates.rs）；
//! 这里只负责把后端返回的元数据还原成插件里的 `Update` 对象，后续下载 / 安装
//! 继续用 `update_download_resumed` / `update_install_resumed`（靠 rid 取回）。

import { invoke } from '@tauri-apps/api/core'
import { Update } from '@tauri-apps/plugin-updater'

export type UpdateChannel = 'stable' | 'beta'

/** 后端 `update_check` 的返回（字段名与 Rust 侧约定一致，勿改） */
export interface UpdateCheckResult {
  rid: number
  currentVersion: string
  version: string
  date?: string | null
  body?: string | null
  rawJson: Record<string, unknown>
  /** 这次结果来自哪个通道 */
  channel: UpdateChannel
}

/**
 * 检查更新。
 *
 * @param channel 生效通道：stable 只看稳定清单；beta 会同时看 beta 与稳定清单
 * @param timeout 单个端点的超时（毫秒），后端默认 10s
 */
export async function checkUpdateForChannel(
  channel: UpdateChannel,
  timeout: number,
): Promise<Update | null> {
  const result = await invoke<UpdateCheckResult | null>('update_check', { channel, timeout })
  if (!result) return null
  // 用插件导出的 Update 类包一层：它自带 close()（释放后端 resource table 里的 update），
  // 也和 `pending.download()` 等原有类型兼容。
  return new Update({
    rid: result.rid,
    currentVersion: result.currentVersion,
    version: result.version,
    date: result.date ?? undefined,
    body: result.body ?? undefined,
    rawJson: result.rawJson,
  })
}
```

- [ ] **Step 2: `src/api/app.ts` 加 `isPrereleaseBuild`**

找到 `getAppVersion` 函数（大概是文件里唯一读版本号的地方），在它后面加：

```ts
/** 当前安装的是不是内测版：版本号里带预发布后缀（如 1.0.25-beta.1） */
export async function isPrereleaseBuild(): Promise<boolean> {
  return (await getAppVersion()).includes('-')
}
```

- [ ] **Step 3: 改 `src/composables/useUpdater.ts` 的导入**

把文件顶部的

```ts
import { ref, computed } from 'vue'
import { Channel, invoke } from '@tauri-apps/api/core'
import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import { isDevBuild } from '@/api/app'
import { safeGet, safeSet } from './safeStorage'
```

改成：

```ts
import { ref, computed } from 'vue'
import { Channel, invoke } from '@tauri-apps/api/core'
import type { Update } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import { isDevBuild, isPrereleaseBuild } from '@/api/app'
import { checkUpdateForChannel, type UpdateChannel } from '@/api/updater'
import { safeGet, safeSet } from './safeStorage'
```

- [ ] **Step 4: 改 `PendingUpdate` 类型**

把

```ts
type PendingUpdate = NonNullable<Awaited<ReturnType<typeof check>>>
```

改成：

```ts
type PendingUpdate = Update
```

- [ ] **Step 5: 加通道状态**

找到 `const dismissedVersion = ref(safeGet(DISMISS_KEY) ?? '')`，在它后面加：

```ts
/** 当前生效的更新通道：由 setChannel 按「配置偏好 / 安装版本」算出来 */
const channel = ref<UpdateChannel>('stable')
/** 配置里的通道偏好：'' = 跟随安装版本（预发布版按 beta）；'stable' / 'beta' = 用户显式选择 */
const channelPreference = ref<'' | UpdateChannel>('')
```

- [ ] **Step 6: `checkUpdate` 改走新命令**

把 `checkUpdate` 里的

```ts
    const update = await check({ timeout: CHECK_TIMEOUT_MS })
```

改成：

```ts
    const update = await checkUpdateForChannel(channel.value, CHECK_TIMEOUT_MS)
```

- [ ] **Step 7: 加 `setChannel`**

在 `dismissCurrent` 之后、`checkUpdate` 之前加：

```ts
/**
 * 设置通道偏好并计算生效通道。
 *
 * 空串 = 跟随安装版本：内测版（版本号带 `-`）自动按 beta，正式版按 stable。
 * 只有生效通道真的变了才清状态，避免每次启动都白丢一次检查结果。
 * 注意这里不主动检查：调用方（App.vue 启动 / 设置页切换）自己决定何时查，
 * 否则启动时会查两遍。
 */
async function setChannel(preference: '' | UpdateChannel) {
  channelPreference.value = preference
  const effective: UpdateChannel =
    preference === '' ? ((await isPrereleaseBuild()) ? 'beta' : 'stable') : preference
  const changed = effective !== channel.value
  channel.value = effective
  if (!changed) return
  closePending()
  reset()
  // 换了通道，之前的「忽略此版本」不应该继续压住新通道的提示
  dismissedVersion.value = ''
  void safeSet(DISMISS_KEY, '')
}
```

- [ ] **Step 8: 暴露通道 API**

在靠近文件末尾的 `const updaterApi = {` 对象里，`devBuild,` 之后加：

```ts
  channel,
  channelPreference,
  setChannel,
```

- [ ] **Step 9: 类型检查 + 构建**

```bash
pnpm build 2>&1 | tail -10
```

Expected：退出码 0。

- [ ] **Step 10: 提交**

```bash
git add src/api/updater.ts src/api/app.ts src/composables/useUpdater.ts
git commit -m "feat(update): 前端按通道检查更新"
```

---

### Task 5: 设置页开关 + App.vue 接线与内测标识

**Files:**
- Modify: `src/components/settings/SettingsPanel.vue`
- Modify: `src/App.vue`

- [ ] **Step 1: SettingsPanel 加 props / emits / 组件导入**

把

```ts
import { computed, onMounted, ref, watch } from 'vue'
```

改成：

```ts
import { computed, onMounted, ref, watch } from 'vue'
import { Modal, Switch } from 'ant-design-vue'
```

把

```ts
const props = defineProps<{
  defaultUploadDir: string | null
  /** 当前工具页是否激活（App.vue 的 view === 'settings'）；设置页是 v-show 常驻挂载的 */
  active: boolean
}>()

const emit = defineEmits<{
  updateDefaultUploadDir: [dir: string | null]
}>()
```

改成：

```ts
const props = defineProps<{
  defaultUploadDir: string | null
  /** 当前生效的更新通道（App.vue 从 useUpdater 拿）；开关只反映它 */
  updateChannel: 'stable' | 'beta'
  /** 当前工具页是否激活（App.vue 的 view === 'settings'）；设置页是 v-show 常驻挂载的 */
  active: boolean
}>()

const emit = defineEmits<{
  updateDefaultUploadDir: [dir: string | null]
  changeUpdateChannel: [channel: 'stable' | 'beta']
}>()
```

- [ ] **Step 2: SettingsPanel 加切换处理**

在「检查更新」那一节的计算属性之前（`// ========== 检查更新 ==========` 注释之后、`const { status: updateStatus ...` 之前）加：

```ts
/** 切到内测通道要确认一次：内测版是给新功能抢先体验的，可能不稳定 */
function handleChannelChange(checked: boolean) {
  if (!checked) {
    emit('changeUpdateChannel', 'stable')
    return
  }
  Modal.confirm({
    title: '开启内测通道？',
    content:
      '开启后优先收到内测版（版本号带 -beta）：新功能会先在內测通道发布，但可能不稳定。' +
      '关闭内测不会降级，当前内测版会保留，等下一个正式版发布后自动回到稳定版。',
    okText: '开启内测',
    cancelText: '取消',
    onOk: () => emit('changeUpdateChannel', 'beta'),
  })
}
```

- [ ] **Step 3: SettingsPanel 模板加「更新通道」行**

在「检查更新」卡片里，`当前版本` 那一行之后加：

```html
        <div class="info-row">
          <span class="info-label">更新通道</span>
          <span class="info-value channel-value">
            <Switch
              :checked="updateChannel === 'beta'"
              size="small"
              @change="(checked) => handleChannelChange(!!checked)"
            />
            <span>{{ updateChannel === 'beta' ? '内测版' : '稳定版' }}</span>
            <span v-if="updateChannel === 'beta'" class="channel-beta-tag">beta</span>
          </span>
        </div>
```

> 位置：`<div class="info-grid">` 里紧跟 `当前版本` 的 `info-row`，在 `v-if="latestVersionLabel"` 的
> 「最新版本」行之前。

- [ ] **Step 4: SettingsPanel 加样式**

在文件末尾的 `<style scoped>` 里加：

```css
/* 更新通道：开关 + 文字 + beta 小标签 */
.channel-value {
  display: flex;
  align-items: center;
  gap: 8px;
}
.channel-beta-tag {
  display: inline-flex;
  align-items: center;
  height: 16px;
  padding: 0 6px;
  border-radius: 8px;
  background: var(--bg-tag);
  color: var(--text-tag);
  font-size: 10px;
  font-weight: 600;
  line-height: 1;
}
```

- [ ] **Step 5: App.vue 导入 / 状态**

把

```ts
import { getAppName, isDevBuild } from "@/api/app";
```

改成：

```ts
import { getAppName, isDevBuild, isPrereleaseBuild } from "@/api/app";
```

在 `const devBuild = ref(false);` 那个 `void isDevBuild().then(...)` 块之后加：

```ts
/** 当前安装的是内测版：标题栏更新按钮旁边挂 beta 标签 */
const isPrerelease = ref(false);
```

- [ ] **Step 6: App.vue 接 useUpdater 的新字段**

把

```ts
const {
  status: updateStatus,
  info: updateInfo,
  errorMessage: updateError,
  percentage: updatePercentage,
  checkUpdate,
  installUpdate,
  restartApp,
  startDailyCheck,
  stopDailyCheck,
} = useUpdater();
```

改成：

```ts
const {
  status: updateStatus,
  info: updateInfo,
  errorMessage: updateError,
  percentage: updatePercentage,
  channel: updateChannel,
  checkUpdate,
  installUpdate,
  restartApp,
  setChannel,
  startDailyCheck,
  stopDailyCheck,
} = useUpdater();
```

- [ ] **Step 7: App.vue 启动检查改成「配置加载完再查」**

把这两段

```ts
// 打开应用先静默检查一次；之后每天 10:00 / 16:00 自动校验。
// 开发构建一律不查：裸二进制装不了更新（Windows 上还会把 dev 可执行文件覆盖成发布版），
// 而且开发版和生产版同时开着时没必要两边都去 GitHub 拉清单。
onMounted(async () => {
  if (await isDevBuild()) return;
  await checkUpdate({ silent: true });
});
onMounted(async () => {
  if (await isDevBuild()) return;
  startDailyCheck();
});
onUnmounted(stopDailyCheck);
```

改成：

```ts
// 更新通道来自 config，必须等配置加载完才能决定（空值时要看安装版本是不是 beta），
// 所以启动检查放在 loading 由 true 变 false 之后；否则预发布版可能先按稳定通道查一次。
let updaterInitialized = false;
async function initUpdater() {
  await setChannel(config.value.update_channel ?? "");
  isPrerelease.value = await isPrereleaseBuild();
  // 开发构建一律不查：裸二进制装不了更新（Windows 上会把 dev 可执行文件覆盖成发布版）
  if (await isDevBuild()) return;
  await checkUpdate({ silent: true });
  startDailyCheck();
}
watch(loading, (value) => {
  if (value || updaterInitialized) return;
  updaterInitialized = true;
  void initUpdater();
});
onUnmounted(stopDailyCheck);
```

- [ ] **Step 8: App.vue 加通道切换处理**

在 `handleUpdateAction` 函数之后加：

```ts
/** 设置页切通道：先落盘，再按新通道立刻检查一次 */
async function handleChangeUpdateChannel(next: "stable" | "beta") {
  config.value.update_channel = next;
  try {
    await saveConfig();
  } catch (err) {
    message.error(`保存更新通道失败: ${err}`);
  }
  await setChannel(next);
  if (await isDevBuild()) return;
  await checkUpdate({ silent: true });
}
```

- [ ] **Step 9: App.vue 模板接线 + beta 标签**

把

```html
          <SettingsPanel
            :active="view === 'settings'"
            :default-upload-dir="config.default_upload_dir"
            @update-default-upload-dir="handleSetDefaultUploadDir"
          />
```

改成：

```html
          <SettingsPanel
            :active="view === 'settings'"
            :default-upload-dir="config.default_upload_dir"
            :update-channel="updateChannel"
            @update-default-upload-dir="handleSetDefaultUploadDir"
            @change-update-channel="handleChangeUpdateChannel"
          />
```

标题栏里，把

```html
        <span v-if="devBuild" class="titlebar-dev-tag" title="开发构建">dev</span>
```

改成：

```html
        <span v-if="isPrerelease" class="titlebar-dev-tag titlebar-beta-tag" title="内测版">beta</span>
        <span v-if="devBuild" class="titlebar-dev-tag" title="开发构建">dev</span>
```

- [ ] **Step 10: App.vue 加 beta 标签样式**

在 `.titlebar-dev-tag` 那条样式之后加：

```css
/* 内测版标识：和 dev 标签同形，换成主色实心 */
.titlebar-beta-tag {
  background: var(--primary);
  color: #fff;
}
```

- [ ] **Step 11: 构建验证**

```bash
pnpm build 2>&1 | tail -10
```

Expected：退出码 0；`vue-tsc` 没有报类型错误。

- [ ] **Step 12: 提交**

```bash
git add src/components/settings/SettingsPanel.vue src/App.vue
git commit -m "feat(settings): 内测通道开关与标题栏 beta 标识"
```

---

### Task 6: `gitee-sync.mjs` —— `--beta` 只发 Gitee

**Files:**
- Modify: `scripts/gitee-sync.mjs`

- [ ] **Step 1: 加 beta 开关与清单路径**

在文件顶部的参数解析区（`const DRY_RUN = process.argv.includes('--dry-run')` 附近）加：

```js
/** beta 通道：产物和清单只发 Gitee，不碰 GitHub，也不读 GitHub 清单 */
const BETA = process.argv.includes('--beta')
/** beta 清单在 Gitee 仓库里的路径（相对仓库根）；稳定清单仍是 updates/latest.json */
const BETA_MANIFEST = `${UPDATES_DIR}/beta/latest.json`
```

- [ ] **Step 2: 加「读 Gitee 清单」与「合并 beta 清单」**

在 `writeGiteeManifest` 之前加：

```js
/** 读 Gitee 仓库里的清单；不存在（404/400）时返回 null。 */
async function readGiteeManifest(branch, relPath) {
  try {
    const data = await gitee(`/repos/${GITEE_REPO}/contents/${relPath}`, { query: { ref: branch } })
    if (!data || !data.content) return null
    return JSON.parse(Buffer.from(data.content, 'base64').toString('utf8'))
  } catch (err) {
    if (err.status === 404 || err.status === 400) return null
    throw err
  }
}

/**
 * 合并 beta 清单（与 spec 5.2 一致）：
 * - 同版本 = 这次是「补齐平台」，保留其它平台已有条目；
 * - 版本变了 = 只保留这次构建的平台，避免 beta.14 的清单里混进 beta.13 的签名/地址，
 *   否则客户端会「提示 beta.14、下载 beta.13」，版本号不变又反复提示。
 */
function mergeBetaManifest(current, partial) {
  const sameVersion = Boolean(current && current.version === partial.version)
  const platforms = sameVersion
    ? { ...(current.platforms || {}), ...(partial.platforms || {}) }
    : { ...(partial.platforms || {}) }
  return {
    version: partial.version,
    notes: partial.notes || `ws-workbench v${partial.version} (beta)`,
    pub_date: partial.pub_date || new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
    platforms,
  }
}
```

- [ ] **Step 3: `writeGiteeManifest` 支持指定路径**

把

```js
/** 写 Gitee 仓库里的清单：有就 PUT（update，需要 sha），没有就 POST（create）。 */
async function writeGiteeManifest(branch, manifest, label) {
  const content = Buffer.from(JSON.stringify(manifest, null, 2) + '\n').toString('base64')
  const path = `/repos/${GITEE_REPO}/contents/${UPDATES_DIR}/${ASSET_MANIFEST}`
```

改成：

```js
/** 写 Gitee 仓库里的清单：有就 PUT（update，需要 sha），没有就 POST（create）。 */
async function writeGiteeManifest(
  branch,
  manifest,
  label,
  relPath = `${UPDATES_DIR}/${ASSET_MANIFEST}`
) {
  const content = Buffer.from(JSON.stringify(manifest, null, 2) + '\n').toString('base64')
  const path = `/repos/${GITEE_REPO}/contents/${relPath}`
```

并把函数最后一行

```js
  log(`已写入 Gitee 清单 ${UPDATES_DIR}/${ASSET_MANIFEST}（${label}）`)
```

改成：

```js
  log(`已写入 Gitee 清单 ${relPath}（${label}）`)
```

- [ ] **Step 4: 加 `runBeta`**

在 `async function main()` 之前加：

```js
/**
 * beta 通道发布：只写 Gitee。
 *
 * 输入是 release.mjs 生成的「只含本平台」清单（--manifest）：签名在那边从 .sig 读好，
 * 这里负责和 Gitee 上已有的 beta 清单按版本规则合并、上传本平台产物、写回清单。
 * 平台补齐（Windows / Mac Intel）用同一个 --version 再跑一次即可。
 */
async function runBeta() {
  if (!DRY_RUN && !GITEE_TOKEN) fail('beta 发布需要 GITEE_TOKEN（Gitee 私人令牌）')

  const partial = readBaseManifest()
  const version = (VERSION || partial.version || '').trim()
  if (!/^\d+\.\d+\.\d+-beta\.\d+$/.test(version)) {
    fail(`beta 版本号必须是 X.Y.Z-beta.N 形式: ${version}`)
  }
  if (partial.version !== version) {
    fail(`清单版本(${partial.version})和指定版本(${version})不一致`)
  }

  const branch = await resolveBranch()
  const tag = `v${version}`
  const baseUrl = `${downloadBase()}/${tag}`
  const remote = await readGiteeManifest(branch, BETA_MANIFEST)
  const { found: assets, installers } = locateAssets(version)

  if (DRY_RUN) {
    log(`[dry-run] beta 版本 ${version}，合并后平台: ${Object.keys(partial.platforms || {}).join(', ')}`)
    for (const [key, entry] of Object.entries(partial.platforms || {})) {
      log(`[dry-run] ${key}: ${entry.url || '（清单里没有 url）'}`)
    }
    return
  }

  log(`beta 版本: ${tag}   清单分支: ${branch}`)
  const release = await ensureRelease(branch, tag, partial)

  // 1) 按清单里的平台逐个上传并用下载地址校验；失败的平台不写进清单（宁可少一个平台，
  //    也不能让清单指向 404）
  const platforms = {}
  const failures = []
  for (const [key, entry] of Object.entries(partial.platforms || {})) {
    const platform = PLATFORMS.find((item) => item.key === key)
    const local = assets[key]
    if (!platform || !local) {
      failures.push(`${key}: 缺少构建产物`)
      continue
    }
    const size = statSync(local).size
    try {
      await uploadAttachment(release, local, platform.asset)
      await verifyRemote(`${baseUrl}/${platform.asset}`, size)
      platforms[key] = { ...entry, url: `${baseUrl}/${platform.asset}` }
      log(`\x1b[32m${key} 已发布\x1b[0m → ${baseUrl}/${platform.asset}`)
    } catch (err) {
      failures.push(`${key}: ${err.message}`)
      warn(`${key} 上传/校验失败: ${err.message}`)
    }
  }

  // 2) 手工安装包：只是发行版附件，失败不影响更新清单
  for (const installer of installers) {
    try {
      await uploadAttachment(release, installer.path, installer.name)
      log(`${installer.label} 已上传: ${installer.name}`)
    } catch (err) {
      warn(`${installer.label} 上传失败（不影响更新清单）: ${err.message}`)
    }
  }

  // 3) 合并并写回 Gitee 的 beta 清单
  const merged = mergeBetaManifest(remote, { ...partial, platforms })
  await withRetry(
    () => writeGiteeManifest(branch, merged, `beta ${tag}`, BETA_MANIFEST),
    '写入 Gitee beta 清单'
  )
  log(
    `客户端 beta 清单: ${GITEE_WEB}/${GITEE_REPO}/raw/${branch}/${BETA_MANIFEST}` +
      `（平台: ${Object.keys(merged.platforms).join(', ')}）`
  )

  if (failures.length > 0) {
    console.error(
      `\x1b[31m[gitee-sync] beta 发布有平台失败（这些平台没有写进清单）：\n  ${failures.join('\n  ')}\x1b[0m`
    )
    process.exitCode = 1
  }
}
```

- [ ] **Step 5: `main()` 分派**

把

```js
async function main() {
  if (!DRY_RUN && !GITEE_TOKEN) fail('缺少 GITEE_TOKEN（Gitee 私人令牌）；CI 里应配置 secrets.GITEE_TOKEN')
```

改成：

```js
async function main() {
  if (BETA) {
    await runBeta()
    return
  }

  if (!DRY_RUN && !GITEE_TOKEN) fail('缺少 GITEE_TOKEN（Gitee 私人令牌）；CI 里应配置 secrets.GITEE_TOKEN')
```

- [ ] **Step 6: 语法检查 + dry-run**

```bash
node --check scripts/gitee-sync.mjs && echo OK
node scripts/gitee-sync.mjs --beta --version 1.0.25-beta.1 --dir release --manifest release/beta-latest.json --dry-run
```

Expected：`OK`；第二条在 `release/beta-latest.json` 还不存在时会报 ENOENT（正常，Task 7 生成它）。
语法检查必须通过。

- [ ] **Step 7: 提交**

```bash
git add scripts/gitee-sync.mjs
git commit -m "feat(release): gitee-sync 支持 beta 通道"
```

---

### Task 7: `release.mjs` —— `--beta` 本地发内测版

**Files:**
- Modify: `scripts/release.mjs`
- Modify: `package.json`

- [ ] **Step 1: 加 Gitee 配置与 `--beta` 开关**

在 `const GITHUB_UPLOAD = 'https://uploads.github.com'` 之后、`const require = createRequire(...)` 之前加：

```js
/** beta 通道只发 Gitee：仓库与 API 地址和 gitee-sync.mjs 保持同一套环境变量 */
const GITEE_REPO = process.env.GITEE_REPO || 'feng_wenshan/ws-workbench'
const GITEE_TOKEN = process.env.GITEE_TOKEN || ''
const GITEE_API = process.env.GITEE_API_BASE || 'https://gitee.com/api/v5'
const GITEE_WEB = process.env.GITEE_WEB_BASE || 'https://gitee.com'
/** beta 通道：构建当前平台 → 写 Gitee beta 清单；完全不碰 GitHub */
const BETA = process.argv.includes('--beta')
```

- [ ] **Step 2: `VERSION_OVERRIDE` 改成可重新赋值**

```js
const VERSION_OVERRIDE = argValue('--version') || process.env.RELEASE_VERSION || ''
```

改成：

```js
let VERSION_OVERRIDE = argValue('--version') || process.env.RELEASE_VERSION || ''
```

- [ ] **Step 3: 加「读 base 版本」**

在 `readVersion()` 之后加：

```js
/** tauri.conf.json 里的版本号，不受 --version 影响：beta 线的 base 版本（如 1.0.25） */
function readConfVersion() {
  const conf = readConf()
  if (!conf.version) fail('tauri.conf.json 里没有 version 字段')
  return conf.version
}
```

- [ ] **Step 4: 加 Gitee API 与 beta 版本解析**

在 `async function main()` 之前加：

```js
// ===== beta 通道（只发 Gitee） =====

/** 调 Gitee Open API（只读接口够用） */
async function giteeApi(path) {
  const url = new URL(`${GITEE_API}${path}`)
  if (GITEE_TOKEN) url.searchParams.set('access_token', GITEE_TOKEN)
  const res = await fetch(url, { headers: { 'User-Agent': 'ws-workbench-release' } })
  const text = await res.text()
  if (!res.ok) {
    throw new Error(`Gitee API ${res.status} ${path}: ${text.slice(0, 300)}`)
  }
  return text ? JSON.parse(text) : null
}

/** Gitee 上所有发行版（分页拉全，用来找已用过的 beta 号） */
async function listGiteeReleases() {
  const all = []
  for (let page = 1; page <= 10; page += 1) {
    const batch = (await giteeApi(`/repos/${GITEE_REPO}/releases?per_page=100&page=${page}`)) || []
    all.push(...batch)
    if (batch.length < 100) break
  }
  return all
}

function versionParts(text) {
  return text.split('.').map((part) => Number(part) || 0)
}

/** a > b（只比较 X.Y.Z，忽略预发布后缀） */
function versionGt(a, b) {
  const left = versionParts(a)
  const right = versionParts(b)
  for (let i = 0; i < 3; i += 1) {
    if ((left[i] || 0) !== (right[i] || 0)) return (left[i] || 0) > (right[i] || 0)
  }
  return false
}

/**
 * 解析这次要发的 beta 版本号。
 * - base 来自 tauri.conf.json；N = Gitee 上已有 `v<base>-beta.*` 的最大值 + 1；
 * - 可以用 --version 指定（补齐平台时必须用同一个版本号）；
 * - base 必须大于最新正式版，防止「beta 线版本号被正式版反超」。
 */
async function resolveBetaVersion() {
  const base = readConfVersion()

  // 显式 --version：补齐平台 / 本机只构建时用，不再查 Gitee（离线也能跑）
  if (VERSION_OVERRIDE) {
    if (!/^\d+\.\d+\.\d+-beta\.\d+$/.test(VERSION_OVERRIDE)) {
      fail(`--version 必须是 X.Y.Z-beta.N 形式: ${VERSION_OVERRIDE}`)
    }
    if (!VERSION_OVERRIDE.startsWith(`${base}-beta.`)) {
      fail(`--version 的 base 必须和 tauri.conf.json 一致（${base}）: ${VERSION_OVERRIDE}`)
    }
    return VERSION_OVERRIDE
  }

  const releases = await listGiteeReleases()
  const tags = releases.map((item) => item.tag_name).filter(Boolean)
  const stableVersions = tags.filter((tag) => /^v\d+\.\d+\.\d+$/.test(tag)).map((tag) => tag.slice(1))
  const latestStable = stableVersions.reduce(
    (best, cur) => (best && !versionGt(cur, best) ? best : cur),
    ''
  )
  if (latestStable && !versionGt(base, latestStable)) {
    fail(
      `tauri.conf.json 的版本 ${base} 必须大于最新正式版 ${latestStable}；` +
        `先在 beta 分支把 version 提到 ${latestStable} 之上`
    )
  }

  const prefix = `v${base}-beta.`
  const used = tags
    .filter((tag) => tag.startsWith(prefix))
    .map((tag) => Number(tag.slice(prefix.length)))
    .filter((value) => Number.isFinite(value) && value > 0)
  return `${base}-beta.${used.length > 0 ? Math.max(...used) + 1 : 1}`
}

/** beta 发布前的守卫：签名、令牌、分支、工作区 */
function ensureBetaGuards() {
  if (NO_SIGN) fail('--beta 不能和 --no-sign 一起用：更新通道只接受签名产物')
  if (!SKIP_PUBLISH && !GITEE_TOKEN) fail('beta 发布需要 GITEE_TOKEN（Gitee 私人令牌）')
  const branch = git(['rev-parse', '--abbrev-ref', 'HEAD']).trim()
  // 补齐平台时通常在另一台机器 checkout 同一个提交（detached HEAD），
  // 这种场景必须显式给 --version，保证补的是同一个 beta 号
  if (branch !== 'beta' && !VERSION_OVERRIDE) {
    fail(
      `--beta 必须在 beta 分支上执行（或在任意提交上配合 --version 补齐平台），当前分支: ${branch}`
    )
  }
  if (git(['status', '--porcelain']).trim()) {
    fail('工作区有未提交改动：beta 构建必须对应一个确定的提交，先提交再来')
  }
}

/** beta 发布主流程：构建当前平台 → 写本地 beta 清单 → 调 gitee-sync --beta */
async function releaseBeta() {
  ensureBetaGuards()
  const version = await resolveBetaVersion()
  VERSION_OVERRIDE = version
  log(`beta 版本: ${version}（base 来自 tauri.conf.json）`)

  const product = readConf().productName || 'app'
  const platform = resolvePlatform(product)
  build()

  const { bundlePath, sigPath } = findArtifacts(platform, version)
  if (!sigPath) fail('没有签名文件：beta 通道只接受签名产物')
  const signature = readFileSync(sigPath, 'utf8').trim()
  if (!signature) fail('签名文件为空，构建可能没有正确签名')

  mkdirSync(OUT_DIR, { recursive: true })
  copyFileSync(bundlePath, join(OUT_DIR, platform.assetFile))
  log(`已导出 release/${platform.assetFile}`)
  if (WANT_DMG) createDmg(platform, version)

  const tag = `v${version}`
  const entry = {
    signature,
    url: `${GITEE_WEB}/${GITEE_REPO}/releases/download/${tag}/${platform.assetFile}`,
  }
  const manifest = {
    version,
    notes: `ws-workbench ${tag} (beta)`,
    pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
    platforms: { [platform.key]: entry },
  }
  const localManifest = join(OUT_DIR, 'beta-latest.json')
  writeFileSync(localManifest, JSON.stringify(manifest, null, 2) + '\n')
  log(`已生成 release/beta-latest.json（平台: ${platform.key}）`)

  if (SKIP_PUBLISH) {
    log('已指定 --no-publish，跳过发布（产物只在本机 release/）')
    return
  }

  log('发布到 Gitee …')
  execFileSync(
    process.execPath,
    [
      'scripts/gitee-sync.mjs',
      '--beta',
      '--version',
      version,
      '--dir',
      OUT_DIR,
      '--manifest',
      localManifest,
    ],
    { cwd: ROOT, stdio: 'inherit' }
  )

  const sha = git(['rev-parse', 'HEAD']).trim()
  log(`beta 发布完成: ${tag} (${platform.key})，提交 ${sha}`)
  log('补齐其它平台：在对应机器上 checkout 同一个提交，用同一个 --version 再跑一次：')
  log(`  node scripts/release.mjs --beta --version ${version}`)
  log('（mac 上补 Intel 版再加 --target x86_64-apple-darwin）')
}
```

- [ ] **Step 5: `main()` 分派**

```js
async function main() {
  const version = readVersion()
```

改成：

```js
async function main() {
  if (BETA) {
    await releaseBeta()
    return
  }

  const version = readVersion()
```

- [ ] **Step 6: 加 npm 脚本**

`package.json` 的 `"scripts"` 里，`"release": "node scripts/release.mjs",` 之后加：

```json
    "release:beta": "node scripts/release.mjs --beta",
```

- [ ] **Step 7: 语法检查**

```bash
node --check scripts/release.mjs && echo OK
```

Expected：`OK`。真正的发布验证放在 Task 9（没有 GITEE_TOKEN 时 `--beta` 会在守卫处直接失败，
不会误构建）。

- [ ] **Step 8: 提交**

```bash
git add scripts/release.mjs package.json
git commit -m "feat(release): release.mjs 支持 --beta 内测发布"
```

---

### Task 8: GitHub Actions —— 改为 push `main` + 版本号预检

**Files:**
- Modify: `.github/workflows/release.yml`
- Modify: `scripts/release.mjs`（`ensureGithubRelease` 的 `target_commitish`）

- [ ] **Step 1: 改文件头注释**

把

```
# 创建 tag（vX.Y.Z）才触发发版：校验 tag → 构建安装包 → 上传到 GitHub 发行版
# → 把更新清单提交回 main。直接 push main 不会触发构建。
```

改成：

```
# push main 时触发发版：先跑 preflight 读 tauri.conf.json 的版本号，
# 该版本还没发布过（GitHub 上没有 v<version> 发行版）才继续构建。
# 版本发布过就整条流程跳过，所以合并 beta、提交清单、改文档都不会重复发版。
# 紧急重跑：Actions 里手动 workflow_dispatch 并勾选 force。
```

- [ ] **Step 2: 改「版本号规则」注释**

把

```
# 2) 版本号规则
#    tag 名就是版本号：打 v1.0.17 就发布 1.0.17，通过 tauri build --config
#    注入，不改动源文件，因此不会产生提交回环。
#    tag 必须是 vX.Y.Z 形式，否则预检直接失败。
```

改成：

```
# 2) 版本号规则
#    版本号唯一来源是 src-tauri/tauri.conf.json 的 version。push main 后 preflight
#    读它；release.mjs 再用 tauri build --config 把它注入构建，不改源文件，
#    不会产生提交回环。tag 由 release.mjs 创建（v<version>），不再手工打 tag。
```

- [ ] **Step 3: 改触发条件**

把

```yaml
on:
  push:
    tags:
      - 'v*'
```

改成：

```yaml
on:
  push:
    branches: [main]
  workflow_dispatch:
    inputs:
      force:
        description: 跳过「版本已发布」检查，强制重建当前 tauri.conf.json 版本
        type: boolean
        default: false
```

- [ ] **Step 4: 改并发注释**

把

```yaml
# 同一个 tag 排队执行，避免两次运行并发写同一个发行版
concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false
```

改成：

```yaml
# 同一个分支排队执行，避免两次运行并发写同一个发行版
concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false
```

- [ ] **Step 5: 加 `preflight` job**

在

```yaml
jobs:
  macos:
```

之间插入（`jobs:` 之后、`  macos:` 之前）：

```yaml
jobs:
  # 发版守卫：push main 时读 tauri.conf.json 的版本号，已经发过就跳过后续所有 job。
  # 这样合并 beta、提交更新清单、改文档都不会重复发版。
  preflight:
    runs-on: ubuntu-latest
    outputs:
      should_release: ${{ steps.check.outputs.should_release }}
      version: ${{ steps.check.outputs.version }}
    steps:
      - name: 拉取代码
        uses: actions/checkout@v4

      - name: 读版本并判断是否需要发版
        id: check
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
          if ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
            echo "::error::tauri.conf.json 的 version 必须是 X.Y.Z 形式，当前: $VERSION"
            exit 1
          fi
          echo "version=$VERSION" >> "$GITHUB_OUTPUT"

          FORCE="${{ github.event_name == 'workflow_dispatch' && inputs.force }}"
          if [ "$FORCE" = "true" ]; then
            echo "should_release=true" >> "$GITHUB_OUTPUT"
            echo "手动强制发版：$VERSION"
            exit 0
          fi

          STATUS=$(curl -s -o /dev/null -w '%{http_code}' \
            -H "Authorization: Bearer $GITHUB_TOKEN" \
            -H "Accept: application/vnd.github+json" \
            "https://api.github.com/repos/${GITHUB_REPOSITORY}/releases/tags/v${VERSION}")
          if [ "$STATUS" = "200" ]; then
            echo "should_release=false" >> "$GITHUB_OUTPUT"
            echo "v${VERSION} 已发布过，跳过构建"
          else
            echo "should_release=true" >> "$GITHUB_OUTPUT"
            echo "本次发布版本：${VERSION}"
          fi
```

- [ ] **Step 6: macos job 加依赖**

把

```yaml
  macos:
```

改成：

```yaml
  macos:
    needs: preflight
    if: needs.preflight.outputs.should_release == 'true'
```

（保留 `  macos:` 后面原有的注释。）

- [ ] **Step 7: windows job 加依赖**

把

```yaml
  windows:
```

改成：

```yaml
  windows:
    needs: preflight
    if: needs.preflight.outputs.should_release == 'true'
```

（保留 `  windows:` 后面原有的注释。）

- [ ] **Step 8: gitee-sync job 加依赖**

把

```yaml
  gitee-sync:
    needs: [macos, windows]
```

改成：

```yaml
  gitee-sync:
    needs: [preflight, macos, windows]
    if: needs.preflight.outputs.should_release == 'true'
```

- [ ] **Step 9: 删掉三个 job 里的 tag 校验步骤**

三个 job 里各有一段完全相同的

```yaml
      - name: 校验 tag 并计算版本号
        id: version
        run: |
          REF="${GITHUB_REF_NAME}"
          if ! printf '%s' "$REF" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$'; then
            echo "::error::tag 必须是 vX.Y.Z 形式（例如 v1.0.17），当前: $REF"
            exit 1
          fi
          VERSION="${REF#v}"
          echo "value=$VERSION" >> "$GITHUB_OUTPUT"
          echo "本次发布版本：$VERSION"
```

全部删除（macos / windows / gitee-sync 各一处）。

- [ ] **Step 10: 版本号引用改成 preflight 的输出**

把文件里所有

```yaml
${{ steps.version.outputs.value }}
```

替换成

```yaml
${{ needs.preflight.outputs.version }}
```

一共 5 处（macos：`release.mjs --version`、artifact 名；windows：`release.mjs --version`、artifact 名；
gitee-sync：`gitee-sync.mjs --version`）。

- [ ] **Step 11: 发行版挂到当前提交**

`scripts/release.mjs` 的 `ensureGithubRelease` 里把

```js
        target_commitish: BRANCH,
```

改成：

```js
        // CI 由 push main 触发，显式挂到当前提交：默认分支上并发提交时，
        // 不写这个字段可能把 tag 挂到别的提交。本地发布没有 GITHUB_SHA，退回 BRANCH。
        target_commitish: process.env.GITHUB_SHA || BRANCH,
```

- [ ] **Step 12: 校验 YAML 与脚本**

```bash
ruby -ryaml -e 'YAML.load_file(ARGV[0]); puts "yaml ok"' .github/workflows/release.yml
node --check scripts/release.mjs && echo "js ok"
```

Expected：`yaml ok` + `js ok`。

- [ ] **Step 13: 提交**

```bash
git add .github/workflows/release.yml scripts/release.mjs
git commit -m "ci(release): push main 触发 + 版本号预检"
```

---

### Task 9: 文档、首个内测版演练与收敛

**Files:**
- Modify: `README.md`

- [ ] **Step 1: README 加 beta 命令与流程**

在「## 构建与发布」的 bash 块里，`node scripts/release.mjs` 几行之后加：

```bash
node scripts/release.mjs --beta            # 本地发内测版：构建当前平台 → 发 Gitee（不碰 GitHub）
node scripts/release.mjs --beta --version 1.0.25-beta.3   # 补齐平台：同一个 beta 号在另一台机器再跑
```

在「发布版本的唯一来源是 git tag：…」那段之前，插入一节：

```markdown
### 内测（beta）通道

功能先合到长期分支 `beta`，正式发布 = 把 `beta` 合进 `main`。

- **beta 版本号**：`tauri.conf.json` 的 `version` 是 base（如 `1.0.25`），每次发布自动加成
  `1.0.25-beta.N`（N 取 Gitee 上已有的最大值 + 1），构建时用 `tauri build --config` 注入。
- **发布 beta**：在 `beta` 分支上 `pnpm release:beta`。脚本会构建当前平台、签名、把产物和
  `updates/beta/latest.json` 只发到 Gitee（beta 不进 GitHub、不触发 GitHub 构建）。
  Windows 的 exe/msi、Mac 的 Intel 版在各自机器上 checkout 同一个提交，用同一个
  `--version` 再跑一次，清单会按平台补齐。
- **发布正式版**：`beta` 合进 `main` 后 push。CI 的 preflight 读 `tauri.conf.json` 的版本号，
  该版本没发过才构建（并发/重复 push 不会重复发版）；发完自动打 `v<version>` tag。
  想强制重跑用 Actions 的 `workflow_dispatch` + `force`。
- **客户端**：设置 →「检查更新」→「更新通道」开关。默认关闭即稳定版；打开后收到 beta 版，
  标题栏会挂 `beta` 标签。**关闭不降级**：当前内测版保留，下一个正式版发布后自动收敛
  （semver 里 `1.0.25-beta.1 < 1.0.25`，所以正式版一发布就会被当成更新）。
  内测版（版本号带 `-`）默认就走 beta 通道，`update_channel` 为空时按安装版本自动判断。
- **救火**：beta 清单写坏、或想临时暂停内测，直接在 Gitee 上编辑 / 删除
  `updates/beta/latest.json` 即可：beta 用户下一次检查会自动回退稳定清单，不报错也不降级。
  正式版发坏时，修好代码和 `tauri.conf.json` 版本后 push `main`（preflight 会认为该版本已
  发布而跳过），再用 Actions 的 `workflow_dispatch` + `force` 重建当前版本。
- **顺序要求**：每次正式版发完后，beta 分支要把 `tauri.conf.json` 的版本提到正式版之上，
  否则 beta 线会被正式版反超，内测用户提前「升级」到正式版后就再也收不到 beta。
```

- [ ] **Step 2: 更新「发布版本的唯一来源」段落**

把

```
发布版本的唯一来源是 git tag：CI 把 `vX.Y.Z` 里的 `X.Y.Z` 通过 `tauri build --config` 注入，不改源文件。发布流程：打 tag → 构建（含更新签名）→ 上传安装包到 GitHub 发行版（同名 tag）→ 合并生成本平台的 `updates/latest.json` 条目并提交回 `main` → 串行跑一次 Gitee 镜像（`gitee-sync` job），把三个更新载荷传到 Gitee 发行版并把清单里的下载地址换成 Gitee。客户端先从 Gitee 读清单，读不到再用 GitHub 清单兜底。
```

改成：

```
发布版本的唯一来源是 `src-tauri/tauri.conf.json` 的 `version`：push `main` 后 CI 先读它，
没有同名发行版才构建，构建时通过 `tauri build --config` 注入，不改源文件。发布流程：
push `main` → preflight 判断 → 构建（含更新签名）→ 上传安装包到 GitHub 发行版并打
`v<version>` tag → 合并生成本平台的 `updates/latest.json` 条目并提交回 `main`（带
`[skip ci]`）→ 串行跑一次 Gitee 镜像（`gitee-sync` job），把三个更新载荷传到 Gitee 发行版
并把清单里的下载地址换成 Gitee。客户端先从 Gitee 读清单，读不到再用 GitHub 清单兜底。
```

- [ ] **Step 3: 更新 CI 段落里「发版仍只由 tag 触发」**

把

```
发版仍只由 tag 触发 `release.yml`。
```

改成：

```
发版由 push `main` 触发 `release.yml`，preflight 按 `tauri.conf.json` 版本判断，不受普通提交影响。
```

- [ ] **Step 4: 在 beta 分支实发第一个内测版**

```bash
git status                 # 必须干净
GITEE_TOKEN=... pnpm release:beta
```

Expected 输出（关键行）：

```
[release] beta 版本: 1.0.25-beta.1（base 来自 tauri.conf.json）
[release] 已生成 release/beta-latest.json（平台: darwin-aarch64）
[gitee-sync] beta 版本: v1.0.25-beta.1   清单分支: master
[gitee-sync] darwin-aarch64 已发布 → https://gitee.com/feng_wenshan/ws-workbench/releases/download/v1.0.25-beta.1/ws-workbench_aarch64.app.tar.gz
[gitee-sync] 客户端 beta 清单: https://gitee.com/feng_wenshan/ws-workbench/raw/master/updates/beta/latest.json（平台: darwin-aarch64）
```

- [ ] **Step 5: 装 beta 包做手工验收**

把 `release/` 里的 dmg / exe 装到测试机（覆盖安装正式版），逐项确认：

1. 版本显示 `v1.0.25-beta.1`，标题栏有 `beta` 标签。
2. 设置 → 检查更新 →「更新通道」开关是「内测版」；当前版本 = 最新版本（不提示更新）。
3. 关掉开关 → 再检查：**不降级**，显示已是最新（正式版 1.0.24 比当前低）。
4. 在 beta 上做一个提交（比如 `git commit --allow-empty -m "test: beta.2"`）再
   `pnpm release:beta`，测试机点「检查更新」→ 收到 `1.0.25-beta.2` → 下载/安装成功
   （断点续传、VPN 断开流程照旧）。
5. 打开通道时点开关会弹确认框；取消则开关回到原状态。

- [ ] **Step 6: 模拟 beta 清单缺失（回退稳定清单）**

在 Gitee 网页上把 `updates/beta/latest.json` 暂时改名（或删掉），在测试机点「检查更新」：
Expected：不报错、`update_check` 自动退回稳定清单（当前 beta 版本高于稳定版，显示已是最新）。
查完把文件改回去。

- [ ] **Step 7: 合并到 main 验证收敛**

```bash
git switch main
git pull
git merge --no-ff beta
git push origin main
```

Expected：Actions 跑 release.yml，preflight 输出 `本次发布版本：1.0.25`，三个构建 job 通过，
GitHub 发行版 `v1.0.25` 有资产，`updates/latest.json` 版本变成 1.0.25，Gitee 镜像 job 成功。

然后在测试机（当前 `1.0.25-beta.2`、通道仍开着）点「检查更新」：
Expected：收到 `v1.0.25` 正式版（不再是 beta），安装后版本 `1.0.25`、beta 标签消失。

- [ ] **Step 8: 给下一轮 beta 提 base**

```bash
git switch beta
git merge --no-ff main        # 把刚发的正式版合回 beta，避免分叉
# 改 src-tauri/tauri.conf.json version → 1.0.26
git add src-tauri/tauri.conf.json
git commit -m "chore(release): beta base 提到 1.0.26"
git push origin beta
```

- [ ] **Step 9: 最终全量验证**

```bash
cd src-tauri && cargo test --quiet && cargo check --quiet
cd ..
pnpm build
node --check scripts/release.mjs
node --check scripts/gitee-sync.mjs
ruby -ryaml -e 'YAML.load_file(ARGV[0]); puts "yaml ok"' .github/workflows/release.yml
git status
```

Expected：全绿；`git status` 干净。

- [ ] **Step 10: 提交文档**

```bash
git add README.md docs/superpowers/plans/2026-11-16-beta-update-channel.md
git commit -m "docs: 内测通道使用与发版说明"
```

---

## 自检（写完后逐项核对）

**1. 规格覆盖**

| spec 章节 | 对应任务 |
| --- | --- |
| 5.1 分支/发版流程 | 前置 0.x、Task 7、Task 8、Task 9 |
| 5.2 清单与平台补齐规则 | Task 6（`mergeBetaManifest`）、Task 9 Step 5/7 |
| 5.3 客户端通道检查 | Task 1、Task 2（`choose_channel`）、Task 4 |
| 5.4 设置/UI | Task 5（开关 + 确认 + beta 标签） |
| 5.5 持久化与通道推断 | Task 3（`update_channel`）、Task 5（`setChannel`） |
| 5.6 发布脚本与 CI | Task 6、Task 7、Task 8 |
| 5.7 救火 | Task 8（`workflow_dispatch force`）、README 说明 |
| 5.8 迁移 | 前置 0.2（beta base 1.0.25）、Task 9 Step 7/8 |
| 6 边界 | Task 2（双通道取高/回退）、Task 6（同版本合并、换版本重置）、Task 9 Step 6 |
| 7 测试 | Task 1/2 单测、Task 9 手工验收 |
| 10 改动面 | 全部任务 Files 列表 |

**2. 占位符扫描**：计划里没有 `TODO` / `TBD` / “类似 Task N” 的省略；每个步骤要么有完整代码，
要么是对现有代码的精确替换（老代码原文 + 新代码）。Task 9 的手工验收项是操作步骤，不是占位符。

**3. 类型/命名一致性**

- Rust `UpdateMetadata` 的 `camelCase` 输出 ↔ `src/api/updater.ts` 的 `UpdateCheckResult` 字段一一对应
  （`rid/currentVersion/version/date/body/rawJson/channel`）。
- `UpdateChannel = 'stable' | 'beta'` ↔ `AppConfig.update_channel?: 'stable' | 'beta' | ''`；
  Rust 侧 `update_channel: String`（空串 = 跟随安装版本）。
- `setChannel('')` 用 `isPrereleaseBuild()`（版本号含 `-`）推断；Rust 侧不参与推断，
  只按传入的 `channel` 选清单，避免两处规则不一致。
- `pick_channel` 在 Task 1 定义、Task 2 的 `choose_channel` 复用；不要重复实现。
- `writeGiteeManifest` 第四参数默认值保持原稳定清单路径，老调用点不用改。
- gitee-sync 的 beta 清单路径常量 `BETA_MANIFEST = 'updates/beta/latest.json'`，
  与 Rust 端点派生（`/updates/latest.json` → `/updates/beta/latest.json`）对齐。

**4. 依赖顺序**：Task 1 → 2（`choose_channel` 用到 `pick_channel`）→ 3 → 4 → 5；
Task 6 可独立于 4/5；Task 7 依赖 6；Task 8 依赖 7 的 `--version` 语义；Task 9 依赖全部。

**5. 已知取舍**

- beta 的「补齐平台」不做旧平台条目保留（换版本时清空），符合 spec 5.2 的防「下载旧包」规则。
- `--beta` 不提供 `--no-sign` 逃生口：没签名的 beta 装上去也会被客户端拒收，直接失败更清楚。
- `update_check` 里 beta 检查失败一律静默回退：beta 只是抢先通道，不能因为它挂了让稳定通道也查不了。
- 老客户端依赖的稳定清单地址（`updates/latest.json`，Gitee 与 GitHub 各一份）完全不动：
  Task 6 只给 `writeGiteeManifest` 加了可选的路径参数，默认值仍是稳定清单。

---

## 执行方式

计划已保存到 `docs/superpowers/plans/2026-11-16-beta-update-channel.md`。

选一种继续：

1. **Subagent-Driven（推荐）**：每个 Task 派一个子代理实现，我在任务之间做审查和测试，速度快、上下文干净。
2. **Inline Execution**：我在当前会话里按 Task 顺序直接实现，每完成一个 Task 停下来给你看 diff。
