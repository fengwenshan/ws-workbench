# ws-workbench 安装包自带卸载程序设计

- 日期：2026-11-10
- 分支：main（沿用现有 cleaner / release 脚本）
- 状态：macOS 已实现并验证（2026-11-10）；Windows 计划见 §8，待 Windows 主机执行
- 关联：`2026-11-10-uninstall-window-design.md`

## 1. 背景与目标

「软件卸载」窗口已经可以卸载其它应用，并明确过滤 ws-workbench 自身（不能从列表里卸载自己）。
但 ws-workbench 自己也需要一个**随安装包分发**的卸载入口，效果与常见工具一致：

1. macOS DMG 里附带可双击的卸载程序；Windows 继续使用安装器自带的标准卸载器（设置 → 应用 → 卸载）。
2. 卸载前先让 ws-workbench 退出：先正常退出，退不掉再询问是否强制退出；**所有相关进程归零后才允许删除任何文件**。仍有进程时直接报「卸载失败」，不删应用、不删数据。
3. 删除前把真实的数据/附件路径展示给用户：数据库、配置、**附件上传目录**、剪贴板图片、WebView 缓存等。
4. **默认保留数据**；用户明确选择「删除全部数据」并二次确认后才删除 App 自有数据。用户自定义的附件目录只提醒、不自动删，避免误删共享目录。

## 2. 非目标

- 不在应用内增加「卸载 ws-workbench」按钮（用户确认只做安装包内置卸载程序）。
- 不在「软件卸载」列表里显示 ws-workbench 自身（现有规则不变）。
- 本期不在 Windows 上实现数据清理页；Windows 主机上再按 §8 执行。
- 不引入 root / 特权 helper；卸载程序只以当前用户权限删除自己的应用与数据。

## 3. 架构

```text
ws-workbench.app
  └─ 启动/保存设置时写 ~/Library/Application Support/<数据目录>/uninstall-info.txt
        （纯路径清单，不含密钥、不含配置内容，权限 600）

ws-workbench_<version>_<arch>.dmg
  ├─ ws-workbench.app
  ├─ Applications -> /Applications
  └─ 卸载 ws-workbench.command   ← bash + osascript，读取上面的路径清单
```

- **路径清单（sidecar）**：由 App 写，保证卸载程序能准确显示自定义附件目录；卸载脚本无需读取加密配置或钥匙串。
- **卸载程序**：放在 DMG 根目录，不进入 App bundle，因此删除 App 时不会删到自己；不依赖网络、不依赖 Python。
- **对话框**：`osascript display dialog`，不引入额外 UI 框架。

## 4. 卸载流程

```text
双击「卸载 ws-workbench.command」
  → 定位 /Applications/ws-workbench.app 或 ~/Applications/ws-workbench.app
  → 校验 CFBundleIdentifier == com.ws.workbench
  → 确认「退出并卸载」
  → osascript 正常退出 → 轮询进程归零（最多约 10 秒）
       ├─ 未归零 → 询问「取消 / 强制退出」
       │            强制退出 = SIGTERM → 1 秒后 SIGKILL → 再轮询（最多 3 轮）
       │            仍未归零 → 卸载失败，未删除任何文件
       └─ 归零 → 继续
  → 读取 uninstall-info.txt（找不到则回退默认路径，并提示无法确认自定义附件目录）
  → 确认框列出：应用本体 / 数据目录 / 附件目录 / 剪贴板图片 / 缓存
       [取消] [保留数据并卸载（默认）] [删除全部数据]
       选「删除全部数据」→ 二次确认（附件、数据库删除后不可恢复）
  → 删除前再次检查进程归零（防止确认框期间重新启动）
  → 应用移到废纸篓（失败则中止，数据不动）
  → 若选择删数据：删除 App 自有数据目录、缓存、偏好、剪贴板图片
       自定义附件目录只记录到完成提示，不自动删
  → 完成对话框 + 写日志 ~/Library/Logs/ws-workbench-uninstall.log
```

### 4.1 进程规则

- 只匹配命令行里包含 `/ws-workbench.app/Contents/MacOS/ws-workbench` 的进程；不会误杀同名脚本。
- `--vpn-worker` 也是同一个可执行文件的不同参数进程，会被同一规则匹配到。
- 正常退出失败时，必须由用户明确点「强制退出」才执行 `kill`。
- 任何一步发现仍有进程，都直接中止；**不先删应用、不先删数据**。

### 4.2 数据路径

卸载程序展示以下真实存在或已配置的路径：

- 应用本体：`/Applications/ws-workbench.app`（或 `~/Applications/...`）
- 数据/配置：
  - `~/Library/Application Support/com.ws.workbench`
  - `~/Library/Application Support/ws-workbench`
  - `~/.ws-workbench`
- 数据库：`<数据目录>/ws-workbench.db`
- 附件目录：`<数据目录>/attachments`，或用户在设置里自定义的目录（`default_upload_dir`）
- 剪贴板图片：`<数据目录>/clipboard-attachments`
- 缓存：`~/Library/WebKit/com.ws.workbench*`、`~/Library/Caches/com.ws.workbench*`、
  `~/Library/HTTPStorages/com.ws.workbench*`、`~/Library/Saved Application State/com.ws.workbench.savedState`、
  `~/Library/Preferences/com.ws.workbench.plist`、`~/Library/Logs/ws-workbench*`

### 4.3 删除边界

- 只删除 `$HOME/` 下的已知 App 目录与缓存；拒绝 `/`、`$HOME` 本身、`/Applications`、`/Library` 等。
- 自定义附件目录不在 App 自有目录内时，**不自动删**；在完成提示里给出路径，让用户自行处理。
- 删除数据前已二次确认；应用本身进废纸篓，数据是永久删除（弹窗中写明）。

## 5. 路径清单格式

`uninstall-info.txt`（`uninstall_info::FILE_NAME`）：

```text
version=1
app_bundle=/Applications/ws-workbench.app
data_dir=/Users/me/Library/Application Support/com.ws.workbench
data_dir=/Users/me/Library/Application Support/ws-workbench
db=/Users/me/Library/Application Support/com.ws.workbench/ws-workbench.db
upload_dir=/Users/me/Documents/ws-attachments
upload_custom=1
clipboard_dir=/Users/me/Library/Application Support/com.ws.workbench/clipboard-attachments
cache_dir=/Users/me/Library/WebKit/com.ws.workbench
```

- 每个 `data_dir` / `cache_dir` 一行，卸载脚本按行读取，不做 `eval`。
- 路径含换行时跳过该条，避免清单格式被破坏。
- 写入位置：所有现有数据目录 + `~/Library/Application Support/com.ws.workbench`（必要时创建）。
- 写入时机：App `setup` 完成、`save_settings` 成功后；失败只打日志，不影响主流程。
- 权限：Unix 上 `0600`；内容只有本地路径，无密钥。

## 6. 通用卸载窗口的安全补强

与自卸载同一原则：进程状态不确定时不能继续删。

- `appRunning` 查询抛错时：中止并提示「无法确认应用运行状态，未删除任何文件」，不再按“未运行”继续。
- 强制退出成功后、调用系统卸载前，再查一次运行状态；仍有进程则中止。
- 不改变现有「列表过滤自身 + 命令层 is_self_app 拦截」。

## 7. 测试与验收

- Rust 单测：
  - `.app` 路径解析（`.../ws-workbench.app/Contents/MacOS/ws-workbench`）。
  - 清单渲染/解析：换行路径被跳过、`upload_custom` 正确、`data_dir` 多条。
  - 缓存候选路径只包含已知目录。
- 脚本 dry-run（`WS_UNINSTALL_DRY_RUN=1`，不执行 `rm`/`mv`）：
  - 正常退出 → 保留数据 → 只移动 app。
  - 进程不退 → 失败退出、零删除。
  - 选择删除全部 → 只删 App 自有目录，自定义附件目录保留。
- DMG 构建后挂载：`卸载 ws-workbench.command` 存在且可执行。
- `cargo test --lib`、`cargo check --bins`、`pnpm build` 全绿。

## 8. Windows（后续在 Windows 主机执行）

Windows 使用 Tauri NSIS 安装器自带的标准卸载器（Apps & Features），不额外做应用内入口。

后续增强（需 Windows 实机验证）：

1. 在 `src-tauri/tauri.conf.json` 的 `bundle.windows.nsis` 增加 `installerHooks`，指向
   `src-tauri/nsis/uninstall-hooks.nsh`。
2. `NSIS_HOOK_PREUNINSTALL`：
   - `tasklist /FI "IMAGENAME eq ws-workbench.exe"` 检测进程；有则 `MessageBox MB_YESNO`
     询问强制退出，`taskkill /IM ws-workbench.exe /F`；仍存在则 `Abort`，不删文件。
   - 读取 `$APPDATA\com.ws.workbench\uninstall-info.txt`（App 的 sidecar，Windows 路径），
     展示 `data_dir` / `upload_dir` 路径；`MessageBox MB_YESNO` 询问是否删除用户数据，默认否。
   - 自定义附件目录（不在 `$APPDATA` / `$LOCALAPPDATA` 下）只提示不删。
3. `NSIS_HOOK_POSTUNINSTALL`：清理空目录、写日志。
4. 明确 `installMode: "currentUser"`，保证 `$APPDATA` 指向当前用户，避免提权后删错用户目录。
5. 在 Windows 主机上验证：运行中卸载被拦、保留/删除数据、附件目录提示、Apps & Features 入口。

## 9. 验证记录（macOS，2026-11-10）

- `cargo test --lib`：489 passed / 0 failed / 4 ignored。
- `cargo check --bins`：通过。
- `pnpm build`：通过。
- `bash scripts/macos/test-uninstall.sh`：三条 dry-run 场景通过（保留数据、删除全部、进程不退时零删除）。
- 开发版真实启动：在临时 HOME 下生成两份 `uninstall-info.txt`（`com.ws.workbench` / `ws-workbench`），权限 600；不再把项目内 `.config` 写进清单。
- DMG：`TAURI_BUNDLES=app node scripts/release.mjs --no-publish --dmg` 成功，挂载后确认 `卸载 ws-workbench.command` 可执行、`ws-workbench.app` 与 Applications 链接存在。
- 用构建出的真实 `.app` 跑卸载脚本 dry-run：能识别 bundle id 并走「保留数据」流程。

## 10. 风险与回退

- sidecar 丢失：卸载程序回退默认路径，并提示“无法确认自定义附件目录”，不猜测删除。
- 附件目录自定义到共享目录：只提示不删，避免误删。
- 应用无法正常退出：强制退出仍失败则中止；不会出现“应用删了、数据丢了、进程还在”的半完成状态。
- 回退方式：从 DMG 中移除卸载脚本即可恢复原来的拖拽卸载方式；App 侧 sidecar 写入可独立删除。
