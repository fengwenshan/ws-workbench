# 「用 DeepSeek 打开」并落到左侧工作区 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ws-workbench 的「打开方式」菜单新增 `DeepSeek`，点击后把项目目录写进 DSH 请求文件并唤起 DeepSeek Harness；DSH 侧由随 ws-workbench 分发的 `dsh-open-workspace` 插件消费请求，新增/复用左侧工作区并打开空白新会话。缺插件时 ws-workbench 弹窗确认后自动安装。

**Architecture:** 两条链路：① ws-workbench（Tauri/Rust）负责菜单、请求文件 `$DSH_HOME/open-workspace.json`、DSH desktop profile 的插件安装；② DSH 插件（host 半注册 HTTP 路由消费请求文件，client 半 400ms 轮询后调 `ctx.workspaces.create({path})` + `ctx.uiWorkspace.openWorkspace(id)`）。插件源码用 `include_str!` 编进二进制，dev 与安装包一致。

**Tech Stack:** Rust（Tauri 2、serde_json、chrono、uuid、sysinfo 已有）、Vue 3 + ant-design-vue、DSH 插件（ESM + `window.__ModuleLoader__` bundle）、bash + python3（安装脚本）。

---

## 文件清单

| 文件 | 动作 | 责任 |
| --- | --- | --- |
| `dsh-plugin/dsh-open-workspace/package.json` | 新建 | 插件清单、dsh bundle/client 声明、版本号 |
| `dsh-plugin/dsh-open-workspace/cordis.patch.yml` | 新建 | bundle 插入声明 |
| `dsh-plugin/dsh-open-workspace/lib/index.mjs` | 新建 | host 半：请求文件消费 + `/poll` `/status` 路由 |
| `dsh-plugin/dsh-open-workspace/lib/client.js` | 新建 | client 半：轮询、创建/选中工作区 |
| `dsh-plugin/dsh-open-workspace/README.md` | 新建 | 插件说明与安装方式 |
| `src-tauri/src/commands/deepseek.rs` | 新建 | 请求文件、插件状态/安装/回滚、DSH 运行检测、打开命令 |
| `src-tauri/src/commands/mod.rs` | 修改 | 挂 `deepseek` 模块并重导出命令 |
| `src-tauri/src/commands/openwith.rs` | 修改 | macOS 菜单项 + DeepSeek 专用启动；Windows `other_open` 分支 |
| `src-tauri/src/apps/models.rs` | 修改 | `AppId::DeepSeek` 元数据 |
| `src-tauri/src/lib.rs` | 修改 | 注册 3 个新命令 |
| `src-tauri/build.rs` | 修改 | 插件文件改动触发重编译 |
| `src/api/system.ts` | 修改 | 前端命令与类型 |
| `src/components/OpenWithButton.vue` | 修改 | DeepSeek 安装确认 + 打开流程 |
| `scripts/dsh-plugin-install.sh` | 新建 | 手动/临时 home/CI 安装与卸载 |
| `README.md` | 修改 | 文档补充 |
| `docs/superpowers/specs/2026-11-10-deepseek-workspace-open-design.md` | 已存在 | 设计文档（已提交） |

---

## Task 1: DSH 插件包（纯 JS，零依赖）

**Files:**
- Create: `dsh-plugin/dsh-open-workspace/package.json`
- Create: `dsh-plugin/dsh-open-workspace/cordis.patch.yml`
- Create: `dsh-plugin/dsh-open-workspace/lib/index.mjs`
- Create: `dsh-plugin/dsh-open-workspace/lib/client.js`
- Create: `dsh-plugin/dsh-open-workspace/README.md`

- [ ] **Step 1: 写 `package.json`**

```json
{
  "name": "dsh-open-workspace",
  "description": "把 ws-workbench 写下的「打开工作区」请求交给 DSH：新增或复用左侧工作区并打开空白新会话，零外部依赖",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "main": "lib/index.mjs",
  "files": [
    "lib",
    "cordis.patch.yml",
    "README.md"
  ],
  "exports": {
    ".": {
      "default": "./lib/index.mjs"
    },
    "./client": "./lib/client.js",
    "./cordis.patch.yml": "./cordis.patch.yml",
    "./package.json": "./package.json"
  },
  "keywords": [
    "dsh",
    "dsh-plugin",
    "deepseek-harness",
    "workspace",
    "ws-workbench"
  ],
  "dsh": {
    "bundle": {
      "patch": "./cordis.patch.yml"
    },
    "client": {
      "platform": "web"
    }
  }
}
```

> `private: true` 是有意的：这个插件只随 ws-workbench 分发，不做 npm 发布；pnpm 仍然可以把它当本地依赖。

- [ ] **Step 2: 写 `cordis.patch.yml`**

```yaml
# dsh-open-workspace bundle：向组合挂 host 半（消费 ws-workbench 的打开请求）
- insert:
    - id: dsh-open-workspace
      name: 'dsh-open-workspace'
```

- [ ] **Step 3: 写 host 半 `lib/index.mjs`**

```js
/**
 * dsh-open-workspace —— host 半。
 *
 * ws-workbench 点「DeepSeek」时写 `$DSH_HOME/open-workspace.json`，本插件把
 * 它通过 HTTP 路由交给页面里的 client 半（client 半再调 workspaces 服务）。
 * 请求文件是唯一的跨进程桥：ws-workbench 不知道 DSH 的端口、cookie 和鉴权。
 *
 * 路由只在 DSH 自己的 web server 上暴露（宿主鉴权保护），外部进程碰不到；
 * 所有路由都 try/catch，出问题只影响插件本身，不影响宿主动画。
 */
import { readFileSync, statSync, unlinkSync } from 'node:fs'
import { join } from 'node:path'
import { homedir } from 'node:os'

export const name = 'dsh-open-workspace'
export const inject = ['webServer']

const PLUGIN_VERSION = '0.1.0'
/** 请求有效期：10 分钟没被消费就丢弃，避免重启后莫名打开旧目录 */
const REQUEST_TTL_MS = 10 * 60 * 1000

let lastRequest = null
let lastError = null

function dshHome() {
  const custom = process.env.DSH_HOME
  return custom && custom.trim() ? custom : join(homedir(), '.dsh')
}

function requestPath() {
  return join(dshHome(), 'open-workspace.json')
}

/** 读并消费请求文件；过期 / 非法 / 目录不存在时直接删掉并返回 null。 */
function takeRequest() {
  const file = requestPath()
  let raw
  try {
    raw = readFileSync(file, 'utf8')
  } catch {
    return null
  }
  // 先删再校验：坏请求不能一直卡在文件里反复被读到
  try {
    unlinkSync(file)
  } catch {
    // 删不掉也继续，最多下一轮再读一次
  }
  let request
  try {
    request = JSON.parse(raw)
  } catch {
    return null
  }
  if (!request || typeof request.path !== 'string' || !request.path) return null
  const createdAt = Date.parse(request.createdAt)
  if (!Number.isFinite(createdAt) || Math.abs(Date.now() - createdAt) > REQUEST_TTL_MS) return null
  try {
    if (!statSync(request.path).isDirectory()) return null
  } catch {
    return null
  }
  return {
    requestId: typeof request.requestId === 'string' ? request.requestId : '',
    path: request.path,
    createdAt: request.createdAt,
  }
}

export function apply(ctx) {
  const logger = ctx.logger
  const warn = (...args) => {
    if (logger?.warn) logger.warn('[dsh-open-workspace]', ...args)
    else console.warn('[dsh-open-workspace]', ...args)
  }

  ctx.webServer.register({
    kind: 'exact',
    path: '/dsh-open-workspace/poll',
    handler: (req, res) => {
      try {
        const request = takeRequest()
        if (request) lastRequest = { ...request, takenAt: new Date().toISOString() }
        res.setHeader('Content-Type', 'application/json')
        res.setHeader('Cache-Control', 'no-store')
        res.end(JSON.stringify({ request }))
      } catch (error) {
        lastError = String(error)
        warn('poll failed', error)
        res.setHeader('Content-Type', 'application/json')
        res.end('{"request":null}')
      }
    },
  })

  // 调试路由：DSH 页面里打开 /dsh-open-workspace/status 看插件版本与最近一次请求
  ctx.webServer.register({
    kind: 'exact',
    path: '/dsh-open-workspace/status',
    handler: (req, res) => {
      res.setHeader('Content-Type', 'application/json')
      res.end(JSON.stringify({ version: PLUGIN_VERSION, lastRequest, lastError }))
    },
  })
}
```

- [ ] **Step 4: 写 client 半 `lib/client.js`**

```js
window.__ModuleLoader__.load({
  id: 'dsh-open-workspace',
  factory: (require) => {
    var module = { exports: {} }
    var exports = module.exports

    const POLL_INTERVAL_MS = 400
    const READY_TIMEOUT_MS = 60 * 1000

    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

    /** workspaces 列表就绪前不消费请求：启动恢复期抢先打开会被后续恢复覆盖 */
    function workspacesReady(ctx) {
      try {
        return ctx.workspaces.list.getSnapshot().phase === 'ready'
      } catch {
        return false
      }
    }

    async function handleRequest(ctx, request) {
      const workspace = await ctx.workspaces.create({ path: request.path })
      await ctx.uiWorkspace.openWorkspace(workspace.workspaceId)
      return workspace
    }

    function apply(ctx) {
      let busy = false
      let disposed = false
      const startedAt = Date.now()

      async function tick() {
        if (disposed || busy) return
        if (!workspacesReady(ctx)) {
          // 启动后迟迟没 ready 就放弃本轮（等下一次页面刷新再说）
          if (Date.now() - startedAt > READY_TIMEOUT_MS) disposed = true
          return
        }
        busy = true
        try {
          const response = await fetch('/dsh-open-workspace/poll', { cache: 'no-store' })
          if (response.ok) {
            const payload = await response.json()
            const request = payload && payload.request
            if (request && request.path) {
              await handleRequest(ctx, request)
              console.info('[dsh-open-workspace] 已打开工作区:', request.path)
            }
          }
        } catch (error) {
          console.warn('[dsh-open-workspace]', error)
        } finally {
          busy = false
        }
      }

      ctx.effect(() => {
        const timer = setInterval(tick, POLL_INTERVAL_MS)
        return () => {
          disposed = true
          clearInterval(timer)
        }
      }, 'dsh-open-workspace: poll')
    }

    exports.apply = apply

    return exports
  },
})
```

- [ ] **Step 5: 写插件 README（写清安装与调试）**

```markdown
# dsh-open-workspace

ws-workbench 点「打开方式 → DeepSeek」时用的小插件：读取 `$DSH_HOME/open-workspace.json`
里的目录，把它交给 DSH 的 workspace 服务——左侧没有就新增，有就选中，并打开该工作区的空白新会话。

- 不联网、零外部依赖；host 半只注册两个 GET 路由（`/dsh-open-workspace/poll`、`/dsh-open-workspace/status`）。
- 请求文件 10 分钟内有效；过期、坏 JSON、目录不存在都会被直接丢弃。
- 装好后需要**重启一次** DeepSeek Harness 才会加载。

## 安装

ws-workbench 安装包里带这个插件：缺插件时点「打开方式 → DeepSeek」会弹窗确认后自动安装
（复制到 `$DSH_HOME/profiles/desktop/node_modules/` 和 `$DSH_HOME/plugins/`，并改 desktop
profile 的 `dsh.profile.bundles`；首次安装备份 `package.json`）。

也可以手动装：

```bash
scripts/dsh-plugin-install.sh            # 安装 / 更新（DSH 关掉后执行更稳）
scripts/dsh-plugin-install.sh --uninstall
```

## 调试

重启 DSH 后，在页面里打开 `/dsh-open-workspace/status` 看插件版本和最近一次请求；
没有反应时先看 `$DSH_HOME/open-workspace.json` 是否还在、`$DSH_HOME/profiles/desktop/package.json`
里有没有 `dsh-open-workspace`。
```

- [ ] **Step 6: 语法检查并提交**

Run:
```bash
node --check dsh-plugin/dsh-open-workspace/lib/index.mjs
node --check dsh-plugin/dsh-open-workspace/lib/client.js
```
Expected: 两条命令都没有输出、退出码 0。

```bash
git add dsh-plugin
git commit -m "feat(dsh-plugin): 新增 dsh-open-workspace 插件（host 请求路由 + client 落工作区）"
```

---

## Task 2: Rust 请求文件 / 清单合并 / 状态（先写测试）

**Files:**
- Create: `src-tauri/src/commands/deepseek.rs`
- Modify: `src-tauri/src/commands/mod.rs`

- [ ] **Step 1: 在 `commands/mod.rs` 挂模块**

在 `mod config;` 之后加：

```rust
// 「用 DeepSeek 打开」：请求文件 + 插件安装
mod deepseek;
```

在 `pub use config::*;` 之后加：

```rust
pub use deepseek::*;
```

- [ ] **Step 2: 写失败测试（`deepseek.rs` 先只有测试）**

```rust
//! 「用 DeepSeek 打开」：工作区请求文件 + DSH 插件状态/安装。
//!
//! ws-workbench 与 DSH 只通过 `$DSH_HOME/open-workspace.json` 通信：点菜单时写请求，
//! DSH 里的 `dsh-open-workspace` 插件消费后调 `workspaces.create` +
//! `uiWorkspace.openWorkspace`，落到左侧工作区。插件源码在
//! `dsh-plugin/dsh-open-workspace/`，用 include_str! 编进二进制，安装包里一定有。

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn test_home(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join("deepseek")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("profiles/desktop")).unwrap();
        std::fs::write(
            dir.join("profiles/desktop/package.json"),
            r#"{
  "name": "dsh-profile-desktop",
  "private": true,
  "dsh": {
    "profile": {
      "bundles": ["@deepseek-ai/dsh-base", "dsh-chime"]
    }
  },
  "dependencies": {
    "dsh-chime": "^0.1.3"
  }
}
"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn merge_profile_manifest_adds_plugin_and_bundle() {
        let raw = r#"{"name":"dsh-profile-desktop","private":true,"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base"]}},"dependencies":{}}"#;
        let (next, changed) = merge_profile_manifest(raw).unwrap();
        assert!(changed);
        assert!(manifest_contains_plugin(&next));
        assert!(next.contains("link:../../plugins/dsh-open-workspace"));
    }

    #[test]
    fn merge_profile_manifest_is_idempotent() {
        let raw = r#"{"name":"x","dsh":{"profile":{"bundles":["a"]}},"dependencies":{}}"#;
        let (first, changed) = merge_profile_manifest(raw).unwrap();
        assert!(changed);
        let (second, changed_again) = merge_profile_manifest(&first).unwrap();
        assert!(!changed_again);
        assert_eq!(first, second);
    }

    #[test]
    fn merge_profile_manifest_keeps_other_entries() {
        let raw = r#"{"dsh":{"profile":{"bundles":["dsh-chime"]}},"dependencies":{"dsh-chime":"^0.1.3"}}"#;
        let (next, _) = merge_profile_manifest(raw).unwrap();
        let value: serde_json::Value = serde_json::from_str(&next).unwrap();
        assert_eq!(value["dependencies"]["dsh-chime"], "^0.1.3");
        assert_eq!(value["dsh"]["profile"]["bundles"][0], "dsh-chime");
        assert_eq!(value["dsh"]["profile"]["bundles"][1], "dsh-open-workspace");
    }

    #[test]
    fn merge_profile_manifest_rejects_broken_json() {
        assert!(merge_profile_manifest("{oops").is_err());
    }

    #[test]
    fn bundled_plugin_files_are_complete() {
        assert!(!PLUGIN_FILES.is_empty());
        for (name, content) in PLUGIN_FILES {
            assert!(!content.trim().is_empty(), "{} 是空的", name);
        }
        let package = PLUGIN_FILES
            .iter()
            .find(|(name, _)| *name == "package.json")
            .map(|(_, content)| *content)
            .expect("插件包必须有 package.json");
        let value: serde_json::Value = serde_json::from_str(package).unwrap();
        assert_eq!(value["name"], "dsh-open-workspace");
        assert!(value["dsh"]["bundle"]["patch"].is_string());
        assert_eq!(value["dsh"]["client"]["platform"], "web");
    }

    #[test]
    fn status_reports_missing_plugin() {
        let home = test_home("status-missing");
        let status = status_for_home(&home);
        assert!(status.profile_ready);
        assert!(!status.plugin_installed);
        assert_eq!(status.installed_version, "");
        assert!(!status.needs_update);
        assert_eq!(status.dsh_home, home.display().to_string());
    }

    #[test]
    fn write_open_request_writes_fresh_json() {
        let home = test_home("request");
        let target = home.join("project");
        std::fs::create_dir_all(&target).unwrap();
        write_open_request(&home, &target).unwrap();
        let raw = std::fs::read_to_string(request_path(&home)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["path"], target.display().to_string());
        assert!(value["requestId"].as_str().is_some_and(|id| !id.is_empty()));
        assert!(value["createdAt"].as_str().is_some_and(|t| !t.is_empty()));
    }

    #[test]
    fn write_open_request_rejects_missing_dir() {
        let home = test_home("request-missing");
        assert!(write_open_request(&home, &home.join("nope")).is_err());
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cd src-tauri && cargo test deepseek 2>&1 | tail -20`
Expected: 编译错误（`merge_profile_manifest` 等未定义）。

- [ ] **Step 4: 实现请求文件 / 清单合并 / 状态**

加在测试模块之前：

```rust
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 插件包名，同时是 `dsh.profile.bundles` 里的 id
pub const DSH_PLUGIN_NAME: &str = "dsh-open-workspace";
/// 请求文件名（放在 `$DSH_HOME` 下）
pub const REQUEST_FILE: &str = "open-workspace.json";
/// 请求有效期，和插件 host 半的 TTL 保持一致
const REQUEST_TTL_SECS: i64 = 600;
/// 依赖项写进 profile 清单时的 spec
const PLUGIN_LINK_SPEC: &str = "link:../../plugins/dsh-open-workspace";

/// 随二进制分发的插件文件（相对插件目录）。include_str! 让 dev 和安装包行为一致。
pub(crate) const PLUGIN_FILES: &[(&str, &str)] = &[
    (
        "package.json",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../dsh-plugin/dsh-open-workspace/package.json"
        )),
    ),
    (
        "cordis.patch.yml",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../dsh-plugin/dsh-open-workspace/cordis.patch.yml"
        )),
    ),
    (
        "lib/index.mjs",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../dsh-plugin/dsh-open-workspace/lib/index.mjs"
        )),
    ),
    (
        "lib/client.js",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../dsh-plugin/dsh-open-workspace/lib/client.js"
        )),
    ),
    (
        "README.md",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../dsh-plugin/dsh-open-workspace/README.md"
        )),
    ),
];

/// 插件状态：前端据此决定是否弹安装确认、装完是否提示重启
#[derive(Debug, Clone, Serialize)]
pub struct DeepseekPluginStatus {
    pub dsh_home: String,
    pub profile_ready: bool,
    pub plugin_installed: bool,
    pub installed_version: String,
    pub bundled_version: String,
    pub needs_update: bool,
    pub dsh_running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenWorkspaceRequest {
    pub request_id: String,
    pub path: String,
    pub created_at: String,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
}

/// `$DSH_HOME` 优先，和 DSH 的 `resolveDshHome` 规则一致
pub(crate) fn dsh_home() -> Result<PathBuf, String> {
    if let Some(raw) = std::env::var_os("DSH_HOME") {
        if !raw.is_empty() {
            return Ok(PathBuf::from(raw));
        }
    }
    home_dir()
        .map(|home| home.join(".dsh"))
        .ok_or_else(|| "找不到用户主目录（HOME / USERPROFILE）".to_string())
}

pub(crate) fn profile_dir(home: &Path) -> PathBuf {
    home.join("profiles").join("desktop")
}

pub(crate) fn request_path(home: &Path) -> PathBuf {
    home.join(REQUEST_FILE)
}

fn plugin_store_dir(home: &Path) -> PathBuf {
    home.join("plugins").join(DSH_PLUGIN_NAME)
}

fn plugin_active_dir(home: &Path) -> PathBuf {
    profile_dir(home).join("node_modules").join(DSH_PLUGIN_NAME)
}

fn bundled_version() -> String {
    PLUGIN_FILES
        .iter()
        .find(|(name, _)| *name == "package.json")
        .and_then(|(_, content)| serde_json::from_str::<serde_json::Value>(content).ok())
        .and_then(|value| value.get("version").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "0.0.0".to_string())
}

fn installed_version(home: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(plugin_active_dir(home).join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// profile 清单里是否同时登记了依赖和 bundle 项
pub(crate) fn manifest_contains_plugin(raw: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    let dependency_ok = value
        .get("dependencies")
        .and_then(|deps| deps.get(DSH_PLUGIN_NAME))
        .is_some();
    let bundle_ok = value
        .get("dsh")
        .and_then(|dsh| dsh.get("profile"))
        .and_then(|profile| profile.get("bundles"))
        .and_then(|bundles| bundles.as_array())
        .is_some_and(|bundles| bundles.iter().any(|item| item.as_str() == Some(DSH_PLUGIN_NAME)));
    dependency_ok && bundle_ok
}

/// 把插件依赖和 bundle 项合进 profile 清单；返回（新清单, 是否有改动）。
pub(crate) fn merge_profile_manifest(raw: &str) -> Result<(String, bool), String> {
    let mut value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| format!("DeepSeek 的 profile 清单不是合法 JSON: {err}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "DeepSeek 的 profile 清单不是 JSON 对象".to_string())?;
    let mut changed = false;

    let dependencies = object
        .entry("dependencies")
        .or_insert_with(|| serde_json::json!({}));
    let dependencies = dependencies
        .as_object_mut()
        .ok_or_else(|| "profile 清单里的 dependencies 不是对象".to_string())?;
    if !dependencies.contains_key(DSH_PLUGIN_NAME) {
        dependencies.insert(
            DSH_PLUGIN_NAME.to_string(),
            serde_json::json!(PLUGIN_LINK_SPEC),
        );
        changed = true;
    }

    let dsh = object
        .entry("dsh")
        .or_insert_with(|| serde_json::json!({}));
    let dsh = dsh
        .as_object_mut()
        .ok_or_else(|| "profile 清单里的 dsh 不是对象".to_string())?;
    let profile = dsh
        .entry("profile")
        .or_insert_with(|| serde_json::json!({}));
    let profile = profile
        .as_object_mut()
        .ok_or_else(|| "profile 清单里的 dsh.profile 不是对象".to_string())?;
    let bundles = profile
        .entry("bundles")
        .or_insert_with(|| serde_json::json!([]));
    let bundles = bundles
        .as_array_mut()
        .ok_or_else(|| "profile 清单里的 dsh.profile.bundles 不是数组".to_string())?;
    if !bundles
        .iter()
        .any(|item| item.as_str() == Some(DSH_PLUGIN_NAME))
    {
        bundles.push(serde_json::json!(DSH_PLUGIN_NAME));
        changed = true;
    }

    let mut text = serde_json::to_string_pretty(&value).map_err(|err| err.to_string())?;
    text.push('\n');
    Ok((text, changed))
}

pub(crate) fn status_for_home(home: &Path) -> DeepseekPluginStatus {
    let manifest = profile_dir(home).join("package.json");
    let profile_ready = manifest.is_file();
    let manifest_has_plugin = std::fs::read_to_string(&manifest)
        .map(|raw| manifest_contains_plugin(&raw))
        .unwrap_or(false);
    let installed = installed_version(home);
    let bundled = bundled_version();
    let plugin_installed = profile_ready && manifest_has_plugin && installed.is_some();
    let needs_update = plugin_installed && installed.as_deref() != Some(bundled.as_str());
    DeepseekPluginStatus {
        dsh_home: home.display().to_string(),
        profile_ready,
        plugin_installed,
        installed_version: installed.unwrap_or_default(),
        bundled_version: bundled,
        needs_update,
        dsh_running: dsh_running(),
    }
}

/// 原子写：同目录临时文件 + rename（Windows 上 rename 不覆盖，会先删除目标）
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temp = path.with_file_name(format!(".{file_name}.tmp"));
    {
        let mut file = std::fs::File::create(&temp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    #[cfg(windows)]
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    std::fs::rename(&temp, path)?;
    Ok(())
}

pub(crate) fn write_open_request(home: &Path, target: &Path) -> Result<(), String> {
    if !target.is_dir() {
        return Err(format!("路径不存在或不是目录: {}", target.display()));
    }
    let request = OpenWorkspaceRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        path: target.display().to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let text = serde_json::to_string_pretty(&request).map_err(|err| err.to_string())?;
    write_atomic(&request_path(home), text.as_bytes())
        .map_err(|err| format!("写 DeepSeek 打开请求失败: {err}"))
}

/// DSH 是否在运行（只影响提示文案）：看进程名里有没有 DeepSeek Harness
pub(crate) fn dsh_running() -> bool {
    let system = sysinfo::System::new_all();
    system.processes().values().any(|process| {
        process
            .name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("deepseek harness")
    })
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cd src-tauri && cargo test deepseek 2>&1 | tail -25`
Expected: `test result: ok. 8 passed`（数量以实际为准）。

- [ ] **Step 6: 提交**

```bash
git add src-tauri/src/commands/deepseek.rs src-tauri/src/commands/mod.rs
git commit -m "feat(openwith): DeepSeek 工作区请求文件与插件状态（含单测）"
```

---

## Task 3: Rust 插件安装 / 回滚 / Tauri 命令

**Files:**
- Modify: `src-tauri/src/commands/deepseek.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/build.rs`

- [ ] **Step 1: 先加安装相关测试**

追加到 `deepseek.rs` 的 `tests` 模块：

```rust
    #[test]
    fn install_creates_plugin_files_and_manifest() {
        let home = test_home("install");
        let status = install_to_home(&home).unwrap();
        assert!(status.plugin_installed);
        assert_eq!(status.installed_version, status.bundled_version);
        assert!(!status.needs_update);
        for (name, content) in PLUGIN_FILES {
            let installed =
                std::fs::read_to_string(plugin_active_dir(&home).join(name)).unwrap();
            assert_eq!(&installed, content, "{} 内容不一致", name);
        }
        assert!(plugin_store_dir(&home).join("package.json").is_file());
        let manifest = std::fs::read_to_string(profile_dir(&home).join("package.json")).unwrap();
        assert!(manifest_contains_plugin(&manifest));
        assert!(profile_dir(&home).join("package.json.ws-workbench.bak").is_file());
    }

    #[test]
    fn install_twice_is_idempotent() {
        let home = test_home("install-twice");
        install_to_home(&home).unwrap();
        install_to_home(&home).unwrap();
        let manifest = std::fs::read_to_string(profile_dir(&home).join("package.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        let bundles = value["dsh"]["profile"]["bundles"].as_array().unwrap();
        assert_eq!(
            bundles
                .iter()
                .filter(|item| item.as_str() == Some(DSH_PLUGIN_NAME))
                .count(),
            1
        );
        let deps = value["dependencies"].as_object().unwrap();
        assert_eq!(deps.len(), 1);
    }

    #[test]
    fn install_refuses_without_profile() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join("deepseek")
            .join("no-profile");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let error = install_to_home(&dir).unwrap_err();
        assert!(error.contains("先打开一次 DeepSeek"));
    }

    #[test]
    fn install_leaves_manifest_untouched_on_broken_json() {
        let home = test_home("broken-manifest");
        let manifest = profile_dir(&home).join("package.json");
        std::fs::write(&manifest, "{oops").unwrap();
        assert!(install_to_home(&home).is_err());
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), "{oops");
        assert!(!plugin_store_dir(&home).exists());
    }

    #[test]
    fn backup_is_kept_from_first_install() {
        let home = test_home("backup");
        let manifest = profile_dir(&home).join("package.json");
        let original = std::fs::read_to_string(&manifest).unwrap();
        install_to_home(&home).unwrap();
        let backup = profile_dir(&home).join("package.json.ws-workbench.bak");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), original);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && cargo test deepseek 2>&1 | tail -15`
Expected: 编译错误（`install_to_home` 未定义）。

- [ ] **Step 3: 实现插件文件写入 / 安装 / 启动 / 命令**

加在 `write_open_request` 之后：

```rust
fn write_plugin_files(dir: &Path) -> Result<(), String> {
    for (name, content) in PLUGIN_FILES {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("创建插件目录失败 {}: {err}", parent.display()))?;
        }
        std::fs::write(&path, content)
            .map_err(|err| format!("写插件文件失败 {}: {err}", path.display()))?;
    }
    Ok(())
}

/// 整目录替换：先写临时目录再换名，避免更新到一半把 DSH 启动路径写坏。
fn replace_plugin_dir(dir: &Path) -> Result<(), String> {
    let parent = dir
        .parent()
        .ok_or_else(|| format!("插件目录没有父目录: {}", dir.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("创建插件目录失败 {}: {err}", parent.display()))?;
    let name = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("插件目录名非法: {}", dir.display()))?;
    let staging = parent.join(format!(".{name}.new"));
    let old = parent.join(format!(".{name}.old"));
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&old);
    write_plugin_files(&staging)?;
    let had_old = dir.exists();
    if had_old {
        std::fs::rename(dir, &old)
            .map_err(|err| format!("替换插件目录失败 {}: {err}", dir.display()))?;
    }
    if let Err(err) = std::fs::rename(&staging, dir) {
        if had_old {
            let _ = std::fs::rename(&old, dir);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("替换插件目录失败 {}: {err}", dir.display()));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// 安装/更新插件；profile 清单最后写，失败只回滚清单（插件目录先换也不影响启动）。
pub(crate) fn install_to_home(home: &Path) -> Result<DeepseekPluginStatus, String> {
    let profile = profile_dir(home);
    let manifest = profile.join("package.json");
    if !manifest.is_file() {
        return Err(
            "还没有初始化 DeepSeek 的 desktop profile：请先打开一次 DeepSeek Harness，再回来点「DeepSeek」。"
                .to_string(),
        );
    }
    let raw = std::fs::read_to_string(&manifest)
        .map_err(|err| format!("读 DeepSeek profile 清单失败: {err}"))?;
    let (next, _) = merge_profile_manifest(&raw)?;

    let backup = profile.join("package.json.ws-workbench.bak");
    if !backup.exists() {
        std::fs::copy(&manifest, &backup)
            .map_err(|err| format!("备份 DeepSeek profile 清单失败: {err}"))?;
    }

    replace_plugin_dir(&plugin_store_dir(home))?;
    replace_plugin_dir(&plugin_active_dir(home))?;
    if let Err(err) = write_atomic(&manifest, next.as_bytes()) {
        let _ = std::fs::copy(&backup, &manifest);
        return Err(format!("写 DeepSeek profile 清单失败: {err}"));
    }
    Ok(status_for_home(home))
}
```

在文件末尾加启动与命令（`install_to_home` 之后、`dsh_running` 旁边即可）：

```rust
/// 唤起 / 前置 DeepSeek Harness（不带目录参数；工作区交给插件消费请求文件后打开）
pub(crate) fn launch_deepseek(app: &tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        let bundle = crate::commands::deepseek_bundle_path()
            .ok_or_else(|| "DeepSeek Harness 没有安装".to_string())?;
        let output = std::process::Command::new("/usr/bin/open")
            .arg("-a")
            .arg(&bundle)
            .output()
            .map_err(|err| format!("启动 DeepSeek 失败: {err}"))?;
        if output.status.success() {
            return Ok(());
        }
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!(
            "启动 DeepSeek 失败{}",
            if reason.is_empty() {
                String::new()
            } else {
                format!(": {reason}")
            }
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let ctx = crate::commands::app_context(app);
        let location = crate::apps::locate(crate::apps::AppId::DeepSeek, &ctx, "");
        if !location.available {
            return Err(format!("DeepSeek Harness 没有安装。{}", location.note));
        }
        #[cfg(target_os = "windows")]
        {
            return crate::process::shell_open(Path::new(&location.path))
                .map_err(|err| format!("启动 DeepSeek 失败: {err}"));
        }
        #[cfg(not(target_os = "windows"))]
        {
            std::process::Command::new(&location.path)
                .spawn()
                .map(|_| ())
                .map_err(|err| format!("启动 DeepSeek 失败: {err}"))
        }
    }
}

/// 菜单 / 通用打开路径的入口：校验插件后写请求并启动 DSH
pub(crate) fn open_dir_workspace(app: &tauri::AppHandle, dir: &str) -> Result<(), String> {
    let home = dsh_home()?;
    let status = status_for_home(&home);
    if !status.profile_ready {
        return Err(
            "还没有初始化 DeepSeek 的 desktop profile：请先打开一次 DeepSeek Harness，再回来点「DeepSeek」。"
                .to_string(),
        );
    }
    if !status.plugin_installed || status.needs_update {
        return Err(
            "DeepSeek 工作区插件还没安装或需要更新：请回到「打开方式」重新点「DeepSeek」并按提示安装（装完需重启 DeepSeek）。"
                .to_string(),
        );
    }
    let target = crate::commands::nearest_existing_dir(dir);
    if !target.is_dir() {
        return Err(format!("路径不存在: {dir}"));
    }
    write_open_request(&home, &target)?;
    launch_deepseek(app)
}

#[tauri::command]
pub fn deepseek_plugin_status() -> Result<DeepseekPluginStatus, String> {
    let home = dsh_home()?;
    Ok(status_for_home(&home))
}

#[tauri::command]
pub fn install_deepseek_plugin() -> Result<DeepseekPluginStatus, String> {
    let home = dsh_home()?;
    install_to_home(&home)
}

#[tauri::command]
pub fn open_deepseek_workspace(
    app: tauri::AppHandle,
    path: String,
) -> Result<DeepseekPluginStatus, String> {
    let home = dsh_home()?;
    let target = crate::commands::nearest_existing_dir(&path);
    if !target.is_dir() {
        return Err(format!("路径不存在: {path}"));
    }
    open_dir_workspace(&app, &target.to_string_lossy())?;
    Ok(status_for_home(&home))
}
```

- [ ] **Step 4: 注册命令（`lib.rs`）**

在 `commands::open_with,` 之后加：

```rust
            commands::deepseek_plugin_status,
            commands::install_deepseek_plugin,
            commands::open_deepseek_workspace,
```

- [ ] **Step 5: build.rs 监控插件文件**

在 `fn main() {` 开头加：

```rust
    // 插件文件用 include_str! 编进二进制：改动后必须重新编译
    for file in [
        "../dsh-plugin/dsh-open-workspace/package.json",
        "../dsh-plugin/dsh-open-workspace/cordis.patch.yml",
        "../dsh-plugin/dsh-open-workspace/lib/index.mjs",
        "../dsh-plugin/dsh-open-workspace/lib/client.js",
        "../dsh-plugin/dsh-open-workspace/README.md",
    ] {
        println!("cargo:rerun-if-changed={}", file);
    }
```

- [ ] **Step 6: 跑测试确认通过**

Run: `cd src-tauri && cargo test deepseek 2>&1 | tail -25`
Expected: 全部通过（新加 5 个安装测试 + Task 2 的 8 个）。

- [ ] **Step 7: 提交**

```bash
git add src-tauri/src/commands/deepseek.rs src-tauri/src/lib.rs src-tauri/build.rs
git commit -m "feat(openwith): DeepSeek 插件自动安装/回滚 + Tauri 命令"
```

---

## Task 4: 打开方式菜单与 DeepSeek 专用启动

**Files:**
- Modify: `src-tauri/src/commands/openwith.rs`
- Modify: `src-tauri/src/apps/models.rs`

- [ ] **Step 1: 加 macOS 目录项与测试**

在 `MAC_CATALOG` 的 `finder` 项之后插入：

```rust
    MacApp {
        id: "deepseek",
        name: "DeepSeek",
        bundles: &["DeepSeek Harness.app"],
        fixed: None,
    },
```

在 `openwith.rs` 的 `#[cfg(test)]` 模块里（macOS 部分，现有 `apps[0].id == "finder"` 断言附近）加：

```rust
    /// 「用 DeepSeek 打开」排在访达之后，名字就叫 DeepSeek
    #[test]
    fn mac_catalog_puts_deepseek_after_finder() {
        let ids: Vec<&str> = MAC_CATALOG.iter().map(|spec| spec.id).collect();
        let finder = ids.iter().position(|id| *id == "finder").unwrap();
        let deepseek = ids.iter().position(|id| *id == "deepseek").unwrap();
        assert_eq!(deepseek, finder + 1);
        let spec = MAC_CATALOG.iter().find(|spec| spec.id == "deepseek").unwrap();
        assert_eq!(spec.name, "DeepSeek");
        assert_eq!(spec.bundles, &["DeepSeek Harness.app"]);
    }
```

- [ ] **Step 2: macOS 启动分支**

把 `platform_open` 里的 macOS 分支改成把 `app` 传进去：

```rust
#[cfg(target_os = "macos")]
{
    mac_open(app, app_id, dir)
}
```

把 `mac_open` 签名改为：

```rust
fn mac_open(app: &AppHandle, app_id: &str, dir: &str) -> Result<(), String> {
```

在 `mac_open` 里 `finder` 分支之后加：

```rust
    // DeepSeek 不能把目录当参数传：写请求文件后唤起，工作区由 DSH 插件打开
    if spec.id == "deepseek" {
        return crate::commands::deepseek::open_dir_workspace(app, dir);
    }
```

在 `mac_bundle_path` 之后加（给 `deepseek.rs` 用）：

```rust
#[cfg(target_os = "macos")]
pub(crate) fn deepseek_bundle_path() -> Option<PathBuf> {
    MAC_CATALOG
        .iter()
        .find(|spec| spec.id == "deepseek")
        .and_then(mac_bundle_path)
}
```

- [ ] **Step 3: Windows `AppId::DeepSeek`**

`src-tauri/src/apps/models.rs`：

1. `enum AppId` 里 `Trae,` 之后加：

```rust
    /// DeepSeek Harness 桌面版：点「打开方式 → DeepSeek」时把目录作为工作区交给它
    DeepSeek,
```

2. `ALL` 里 `AppId::Trae,` 之后加 `AppId::DeepSeek,`。
3. `as_str` 加 `AppId::DeepSeek => "deepseek",`。
4. `display_name` 加 `AppId::DeepSeek => "DeepSeek",`。
5. `description` 加 `AppId::DeepSeek => "在 DeepSeek Harness 里打开项目工作区",`。
6. `executable_names` 加：

```rust
            AppId::DeepSeek => &["DeepSeek Harness.exe", "DeepSeek.exe"],
```

7. `registry_keywords` 加 `AppId::DeepSeek => &["DeepSeek Harness"],`。
8. `windows_install_dirs` 加：

```rust
            AppId::DeepSeek => &[
                "DeepSeek Harness",
                "Programs\\DeepSeek Harness",
                "DeepSeek",
                "Programs\\DeepSeek",
            ],
```

9. `macos_paths` 加：

```rust
            AppId::DeepSeek => &[
                "/Applications/DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness",
            ],
```

- [ ] **Step 4: Windows 菜单与启动分支**

`openwith.rs` 的 `DESKTOP_EDITORS` 列表末尾（`("trae", "Trae", crate::apps::AppId::Trae),` 之后）加：

```rust
    ("deepseek", "DeepSeek", crate::apps::AppId::DeepSeek),
```

`other_open` 里，在 `terminal` / `powershell` 分支之后、`let app_id = match ...` 之前加：

```rust
    if app_id == "deepseek" {
        return crate::commands::deepseek::open_dir_workspace(app, dir);
    }
```

- [ ] **Step 5: 跑测试 + 检查**

Run: `cd src-tauri && cargo test openwith models 2>&1 | tail -20`
Expected: 通过（含新的 `mac_catalog_puts_deepseek_after_finder`、`all_ids_have_unique_strings` 等）。

Run: `cd src-tauri && cargo check 2>&1 | tail -10`
Expected: `Finished ...`，无 error。

- [ ] **Step 6: 提交**

```bash
git add src-tauri/src/commands/openwith.rs src-tauri/src/apps/models.rs
git commit -m "feat(openwith): 打开方式菜单加入 DeepSeek（macOS/Windows）"
```

---

## Task 5: 前端安装确认与打开流程

**Files:**
- Modify: `src/api/system.ts`
- Modify: `src/components/OpenWithButton.vue`

- [ ] **Step 1: `src/api/system.ts` 加类型与命令**

在 `openWith` 函数之后加：

```ts
/** DeepSeek 工作区插件状态（后端探测本机 DSH desktop profile） */
export interface DeepseekPluginStatus {
  dsh_home: string
  profile_ready: boolean
  plugin_installed: boolean
  installed_version: string
  bundled_version: string
  needs_update: boolean
  dsh_running: boolean
}

/** 查插件状态：缺插件/版本旧时前端弹确认再装 */
export function deepseekPluginStatus(): Promise<DeepseekPluginStatus> {
  return invoke<DeepseekPluginStatus>('deepseek_plugin_status')
}

/** 安装/更新插件（复制到 DSH profile + 改清单，离线、可回滚） */
export function installDeepseekPlugin(): Promise<DeepseekPluginStatus> {
  return invoke<DeepseekPluginStatus>('install_deepseek_plugin')
}

/** 写打开请求并唤起 DeepSeek（插件消费后新建/选中左侧工作区） */
export function openDeepseekWorkspace(path: string): Promise<DeepseekPluginStatus> {
  return invoke<DeepseekPluginStatus>('open_deepseek_workspace', { path })
}
```

- [ ] **Step 2: `OpenWithButton.vue` 改脚本**

1. import 行改为：

```ts
import { message, Modal } from 'ant-design-vue'
import {
  deepseekPluginStatus,
  installDeepseekPlugin,
  listOpenWithApps,
  openDeepseekWorkspace,
  openWith,
  type DeepseekPluginStatus,
  type OpenWithApp,
} from '@/api/system'
```

2. 在 `run` 函数之后加：

```ts
/** 缺插件/版本旧时弹确认；返回用户是否同意安装 */
function askInstall(status: DeepseekPluginStatus, action: string): Promise<boolean> {
  return new Promise((resolve) => {
    Modal.confirm({
      title: `${action} DeepSeek 工作区插件`,
      content:
        `用 DeepSeek 打开项目需要一个一次性安装的本地小插件：离线、不改 DeepSeek 安装目录、` +
        `首次安装会备份 profile 清单，装完需要退出并重新打开一次 DeepSeek 才生效。` +
        `安装位置：${status.dsh_home}/profiles/desktop 与 ${status.dsh_home}/plugins/dsh-open-workspace。`,
      okText: `${action}插件`,
      cancelText: '取消',
      onOk: () => resolve(true),
      onCancel: () => resolve(false),
    })
  })
}

/** 「打开方式 → DeepSeek」的完整流程：查状态 → 必要时装插件 → 写请求 → 唤起 DSH */
async function runDeepseek() {
  let status = await deepseekPluginStatus()
  if (!status.profile_ready) {
    Modal.warning({
      title: 'DeepSeek 还没初始化',
      content: '请先打开一次 DeepSeek Harness，等它完成初始化后再回来点「DeepSeek」。',
    })
    return
  }

  const action = status.plugin_installed ? '更新' : '安装'
  if (!status.plugin_installed || status.needs_update) {
    const agreed = await askInstall(status, action)
    if (!agreed) return
    status = await installDeepseekPlugin()
    message.success(`DeepSeek 工作区插件已${action}`)
    emit('log', 'info', `DeepSeek 工作区插件已${action}: ${status.installed_version}`)
  }

  const wasRunning = status.dsh_running
  await openDeepseekWorkspace(props.path)
  emit('log', 'info', `用 DeepSeek 打开: ${props.path}`)
  if (wasRunning) {
    Modal.info({
      title: '重启 DeepSeek 后工作区才生效',
      content:
        '插件已就绪。请退出并重新打开 DeepSeek Harness：10 分钟内重启会自动打开这个目录；' +
        '超过 10 分钟的话，重启后再点一次「DeepSeek」即可。',
    })
  }
}
```

3. `run` 函数体改成：

```ts
async function run(app: OpenWithApp) {
  if (!hasPath.value || busy.value) return
  busy.value = true
  open.value = false
  try {
    if (app.id === 'deepseek') {
      await runDeepseek()
    } else {
      await openWith(props.path, app.id)
      emit('log', 'info', `用 ${app.name} 打开: ${props.path}`)
    }
  } catch (err) {
    // 失败时给个可见反馈，否则用户只会觉得「点了没反应」
    message.error(`用 ${app.name} 打开失败: ${err}`)
    emit('log', 'error', `用 ${app.name} 打开失败: ${err}`)
  } finally {
    busy.value = false
  }
}
```

- [ ] **Step 3: 类型检查 + 提交**

Run: `pnpm build 2>&1 | tail -15`
Expected: `vue-tsc --noEmit` 无错误，`vite build` 完成。

```bash
git add src/api/system.ts src/components/OpenWithButton.vue
git commit -m "feat(openwith): 前端「DeepSeek」安装确认与打开流程"
```

---

## Task 6: 安装脚本与文档

**Files:**
- Create: `scripts/dsh-plugin-install.sh`
- Modify: `README.md`

- [ ] **Step 1: 写安装脚本**

```bash
#!/usr/bin/env bash
# 把 dsh-open-workspace 插件装进本机 DeepSeek Harness 的 desktop profile。
#
# 生产用户不需要手动跑：ws-workbench 点「打开方式 → DeepSeek」时会自动安装（弹窗确认）。
# 这个脚本用于：开发机手动安装、CI/临时 DSH_HOME、排障卸载。
#
# 用法：
#   scripts/dsh-plugin-install.sh            # 安装 / 更新
#   scripts/dsh-plugin-install.sh --uninstall
#
# 环境变量：DSH_HOME（默认 ~/.dsh）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/dsh-plugin/dsh-open-workspace"
DSH_HOME="${DSH_HOME:-$HOME/.dsh}"
PROFILE="$DSH_HOME/profiles/desktop"
MANIFEST="$PROFILE/package.json"
BACKUP="$PROFILE/package.json.ws-workbench.bak"
PLUGIN_NAME="dsh-open-workspace"

if [ ! -f "$MANIFEST" ]; then
  echo "找不到 $MANIFEST —— 先打开一次 DeepSeek Harness 让它初始化 desktop profile。" >&2
  exit 1
fi

if [ "${1:-}" = "--uninstall" ]; then
  python3 - "$MANIFEST" "$PLUGIN_NAME" <<'PY'
import json, sys
path, name = sys.argv[1], sys.argv[2]
with open(path) as f:
    data = json.load(f)
data.get("dependencies", {}).pop(name, None)
bundles = data.get("dsh", {}).get("profile", {}).get("bundles", [])
if name in bundles:
    bundles.remove(name)
with open(path, "w") as f:
    json.dump(data, f, ensure_ascii=False, indent=2)
    f.write("\n")
PY
  rm -rf "$PROFILE/node_modules/$PLUGIN_NAME" "$DSH_HOME/plugins/$PLUGIN_NAME"
  echo "已卸载 $PLUGIN_NAME（重启 DeepSeek 生效）。"
  exit 0
fi

[ -f "$BACKUP" ] || cp "$MANIFEST" "$BACKUP"
mkdir -p "$DSH_HOME/plugins" "$PROFILE/node_modules"
rm -rf "$DSH_HOME/plugins/$PLUGIN_NAME" "$PROFILE/node_modules/$PLUGIN_NAME"
cp -R "$SRC" "$DSH_HOME/plugins/$PLUGIN_NAME"
cp -R "$SRC" "$PROFILE/node_modules/$PLUGIN_NAME"

python3 - "$MANIFEST" "$PLUGIN_NAME" <<'PY'
import json, sys
path, name = sys.argv[1], sys.argv[2]
with open(path) as f:
    data = json.load(f)
changed = False
deps = data.setdefault("dependencies", {})
if name not in deps:
    deps[name] = "link:../../plugins/dsh-open-workspace"
    changed = True
profile = data.setdefault("dsh", {}).setdefault("profile", {})
bundles = profile.setdefault("bundles", [])
if name not in bundles:
    bundles.append(name)
    changed = True
if changed:
    with open(path, "w") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
        f.write("\n")
PY

echo "已安装 $PLUGIN_NAME（重启 DeepSeek 生效）。"
```

- [ ] **Step 2: 脚本可执行 + 在临时 home 上自测**

Run:
```bash
chmod +x scripts/dsh-plugin-install.sh
TMP=$(mktemp -d /tmp/dsh-plugin-script.XXXXXX)
mkdir -p "$TMP/profiles/desktop"
cp ~/.dsh/profiles/desktop/package.json "$TMP/profiles/desktop/package.json"
DSH_HOME="$TMP" scripts/dsh-plugin-install.sh
python3 -c "import json;d=json.load(open('$TMP/profiles/desktop/package.json'));print(d['dependencies'],d['dsh']['profile']['bundles'])"
DSH_HOME="$TMP" scripts/dsh-plugin-install.sh --uninstall
python3 -c "import json;d=json.load(open('$TMP/profiles/desktop/package.json'));print(d['dependencies'],d['dsh']['profile']['bundles'])"
```
Expected: 安装后 dependencies 里有 `dsh-open-workspace: link:../../plugins/dsh-open-workspace`、bundles 末尾追加；卸载后都消失。

- [ ] **Step 3: README 补充**

在 README 的「打开方式」相关段落后加一段（按现有文档结构找最近位置；标题用 `### DeepSeek Harness` 或并入对应小节）：

```markdown
### 用 DeepSeek 打开

「打开方式」菜单里的 `DeepSeek` 会把当前项目目录交给 DeepSeek Harness：左侧没有这个目录就
新增工作区，有就直接选中，并打开该工作区的空白新会话。

- 第一次点会提示安装一个本地小插件（随 ws-workbench 安装包分发，离线、可回滚，装完需重启一次
  DeepSeek；首次安装会备份 `~/.dsh/profiles/desktop/package.json`）。
- 插件源码在 `dsh-plugin/dsh-open-workspace/`；临时环境或手动安装用
  `scripts/dsh-plugin-install.sh`，卸载加 `--uninstall`。
- DSH 要至少启动过一次（desktop profile 已初始化），否则会提示先打开一次 DeepSeek。
```

- [ ] **Step 4: 提交**

```bash
git add scripts/dsh-plugin-install.sh README.md
git commit -m "docs+chore: DeepSeek 插件安装脚本与 README"
```

---

## Task 7: 端到端验证

**Files:** 无新增；仅验证。

- [ ] **Step 1: 全量 Rust 测试 + 检查**

Run: `cd src-tauri && cargo test 2>&1 | tail -15 && cargo check 2>&1 | tail -5`
Expected: 所有测试 ok，`Finished ...`。

- [ ] **Step 2: 前端构建**

Run: `pnpm build 2>&1 | tail -8`
Expected: 成功。

- [ ] **Step 3: 临时 DSH_HOME 端到端（不碰真实 profile）**

```bash
TMP=$(mktemp -d /tmp/dsh-e2e.XXXXXX)
mkdir -p "$TMP/profiles"
cp -R ~/.dsh/profiles/desktop "$TMP/profiles/desktop"
rm -rf "$TMP/profiles/desktop/node_modules/dsh-open-workspace"
DSH_HOME="$TMP" scripts/dsh-plugin-install.sh
mv "$TMP/profiles/desktop" "$TMP/profiles/e2e"
# 后台启动，记 PID 和日志；--no-open 不支持就去掉（浏览器自动打开也无妨）
DSH_HOME="$TMP" "/Applications/DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh" \
  --profile e2e --no-open --port 8899 > "$TMP/dsh.log" 2>&1 &
echo $! > "$TMP/dsh.pid"
for i in $(seq 1 30); do
  grep -qoE 'http://127\.0\.0\.1:8899/\?token=[^ ]+' "$TMP/dsh.log" && break
  sleep 1
done
grep -oE 'http://127\.0\.0\.1:8899/\?token=[^ ]+' "$TMP/dsh.log" | head -1
```

用 `tabbit` 技能打开上面打印的带 token URL，确认页面正常加载。

- [ ] **Step 4: 写请求文件，验证工作区新增与复用**

```bash
TESTDIR="$TMP/project"
mkdir -p "$TESTDIR"
python3 - "$TMP" "$TESTDIR" <<'PY'
import json, sys, uuid
from datetime import datetime, timezone
home, path = sys.argv[1], sys.argv[2]
req = {"requestId": str(uuid.uuid4()), "path": path,
       "createdAt": datetime.now(timezone.utc).isoformat()}
with open(home + "/open-workspace.json", "w") as f:
    json.dump(req, f)
PY
sleep 2
python3 - "$TMP" <<'PY'
import json, sys
data = json.load(open(sys.argv[1] + "/storages/workspace.json"))
paths = [w["path"] for w in data["tables"]["workspaces"].values()]
assert sys.argv[1] + "/project" in paths, paths
print("新增工作区 OK")
PY
```

再写同一个目录一次（新的 requestId），确认没有重复工作区：

```bash
python3 - "$TMP" "$TESTDIR" <<'PY'
import json, sys, uuid
from datetime import datetime, timezone
home, path = sys.argv[1], sys.argv[2]
with open(home + "/open-workspace.json", "w") as f:
    json.dump({"requestId": str(uuid.uuid4()), "path": path,
               "createdAt": datetime.now(timezone.utc).isoformat()}, f)
PY
sleep 2
python3 - "$TMP" <<'PY'
import json, sys
data = json.load(open(sys.argv[1] + "/storages/workspace.json"))
paths = [w["path"] for w in data["tables"]["workspaces"].values()]
assert paths.count(sys.argv[1] + "/project") == 1, paths
print("复用工作区 OK")
PY
```

浏览器里再截图确认：左侧工作区列表选中该项目，主区域是空白 New Session。

- [ ] **Step 5: 过期请求 / 非法目录**

```bash
echo '{"requestId":"stale","path":"/tmp","createdAt":"2020-01-01T00:00:00Z"}' > "$TMP/open-workspace.json"
sleep 1
[ ! -f "$TMP/open-workspace.json" ] && echo "过期请求已丢弃"
echo '{"requestId":"bad","path":"/definitely/not/here","createdAt":"2026-11-10T00:00:00Z"}' > "$TMP/open-workspace.json"
sleep 1
[ ! -f "$TMP/open-workspace.json" ] && echo "非法目录已丢弃"
```
Expected: 两行都打印；页面无报错。

- [ ] **Step 6: 关掉临时服务**

```bash
kill "$(cat "$TMP/dsh.pid")" 2>/dev/null || true
sleep 1
rm -rf "$TMP"
```

- [ ] **Step 7: 本机真实安装（不重启 DSH）**

```bash
DSH_HOME="$HOME/.dsh" scripts/dsh-plugin-install.sh
python3 -c "import json;d=json.load(open('$HOME/.dsh/profiles/desktop/package.json'));print(d['dependencies'],d['dsh']['profile']['bundles'])"
```

Expected: 清单里出现插件；**已运行中的 DSH 需要用户退出重开才加载**。把这句写进最终回复。

- [ ] **Step 8: 提交验证记录**

```bash
git status --short
```
Expected: 无未提交的计划内文件（临时目录已在 /tmp，不进仓库）。

---

## 风险与回归清单

- DSH 升级后内部服务（`workspaces` / `uiWorkspace` / `webServer.register`）可能变：
  回归动作 = 用 Task 7 Step 3 的临时 home 再跑一遍；失败时插件只退化为「不消费请求」。
- Windows 未在本机编译验证：交付前至少跑 `cargo check --target x86_64-pc-windows-msvc`
  （若本机没装 target，则在 Windows 机器上跑 Task 4 Step 5 的检查）。
- 用户手动用 DSH 插件管理器装/卸载同插件时，以 pnpm 为准；我们只追加依赖与 bundle，
  不删用户已有项。
