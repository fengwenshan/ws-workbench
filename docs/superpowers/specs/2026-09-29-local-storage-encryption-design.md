# 本地存储加密（SP2）设计

- 日期：2026-09-29
- 状态：待评审
- 关联：`2026-09-29-antd-migration-design.md`（SP1）、SP3「Markdown 分组笔记」

## 1. 背景与目标

应用所有**落盘**的数据目前都是明文：

- 配置：`tauri-plugin-store` 的 `config.json`（键 `app_config`），另有 `config.json` 文件回退副本；
- 前端：`localStorage` 里的更新提示关闭标记（`useUpdater.ts` 的 `DISMISS_KEY`）。

目标：**凡存储在本地的内容一律密文落盘，读取时在进程内解密，界面上始终展示明文。** 该约定同时写入项目规范（见第 9 节）。

### 范围

**纳入**

- `config.json`（Store 插件）+ 文件回退副本
- `localStorage` 中的更新提示标记
- `AppConfig` 内嵌的 VPN 配置（含 `vpn_profiles[].password`）

**不纳入**

- 进程内 `MEMORY_CACHE`（仅内存，不落盘）
- VPN 会话目录 / 日志 / 学习报告等运行期产物（非用户配置数据）

## 2. 现状盘点

| 位置 | 现状 | 本次改动 |
| --- | --- | --- |
| `src-tauri/src/config.rs` | `load_from_store`/`save_to_store`、`load_from_file`/`save_to_file` 四个出入口，明文 JSON | 在出入口套 SM4 信封 |
| `src-tauri/src/commands.rs` / `vpn/commands.rs` | 通过 `config::load_config`/`save_config` 读写配置 | 配置读写**零改动**；仅 VPN 密码路径按第 5.4 节调整 |
| `src-tauri/src/vpn/credential.rs` | 已实现 macOS 钥匙串、Windows DPAPI 的 `protect/unprotect/forget` | 作为主密钥托管的参考实现 |
| `src/composables/useUpdater.ts` | `localStorage` 读写 `DISMISS_KEY` | 改走新的 Tauri 命令 |
| `vpn/models.rs` `VpnProfile.password` | 已是 DPAPI/钥匙串密文（`password_encrypted` 标记），落在 `config.json` 内 | 去掉字段级加密，改随整块配置走 SM4 信封（第 5.4 节） |

### 关键约束

Rust 侧多处内部读写配置（如 `commands.rs::app_context`/`set_app_path`、`vpn/commands.rs::remember_task_dir`）。因此加密**必须放在 Rust 存储边界**，内部消费方继续面对明文 `AppConfig`，接口不变。

## 3. 威胁模型与密钥管理

### 目标

- 防「配置文件被随手拷走/同步走后明文可读」；
- **不引入硬编码密钥**，避免被 SAST 判为 CWE-321 / CWE-798。

### 主密钥

- 首次运行时生成 **32 字节随机主密钥**，交由系统密钥库托管：
  - **macOS**：钥匙串，`service = ws-tools-vault`、`account = ws-tools`。复用 `credential.rs` 的 `security add-generic-password -U -a ws-tools -s <service> -w <hex>` / `find-generic-password ... -w` / `delete-generic-password` 调用方式。
  - **Windows**：用 DPAPI（`CryptProtectData`）保护后写入 `<app_data_dir>/.vault_key`（内容形如 `dpapi:<hex>`），复用 `credential.rs` 的 DPAPI FFI。
- 主密钥**永不**写入 `config.json`、**永不**进源码。
- 密钥分离：`enc_key = SM3(master ‖ "enc")`，`mac_key = SM3(master ‖ "mac")`（两者均为 32 字节摘要）。SM4 只接受 128 位密钥，故加密时取 `enc_key` 前 16 字节（SM3 摘要左截断）；HMAC-SM3 使用完整 32 字节 `mac_key`。
- 提供一次性「重置主密钥」说明：删掉钥匙串条目 / `.vault_key` 后，既有密文不可解，需按第 7 节降级策略处理。

## 4. 算法与信封格式

- **SM4-CBC + 随机 IV + SM3/HMAC 先加密后认证（encrypt-then-MAC）**，避免 ECB 泄露模式，避免「只加密不认证」。
- 流程（加密）：
  1. `iv` = 16 字节随机数；
  2. `ct` = `SM4-CBC-Enc(enc_key, iv, plaintext)`（PKCS#7 填充）；
  3. `mac` = `HMAC-SM3(mac_key, iv ‖ ct)`；
  4. 信封 = `v1.<b64(iv)>.<b64(ct)>.<b64(mac)>`。
- 流程（解密）：拆分 → 常数时间校验 `mac`（`Mac::verify_slice`，不匹配返回 `数据已被篡改`）→ 解密 → 去填充。
- 判定：以 `v1.` 前缀识别密文；不含该前缀者按历史明文处理（见第 7 节）。
- Rust 依赖（纯 Rust，无系统库，RustCrypto 系）：
  - `sm4 = "0.5"`、`cbc = "0.1"`、`sm3 = "0.4"`、`hmac = "0.12"`、`base64 = "0.22"`、`rand = "0.8"`

## 5. Rust 改造点

### 5.1 新增 `src-tauri/src/crypto.rs`

- `key` 子模块：`load_or_create_master_key(app: &AppHandle) -> Result<[u8; 32], String>`（macOS 钥匙串 / Windows DPAPI+文件，见第 3 节；Windows 分支需要 `app_data_dir`，故需 `AppHandle`）。
- `pub fn encrypt_str(app: &AppHandle, plain: &str) -> Result<String, String>`
- `pub fn decrypt_str(app: &AppHandle, envelope: &str) -> Result<String, String>`
- `pub fn is_encrypted(s: &str) -> bool`（`v1.` 前缀判定）
- 单测：往返一致、中文/长文本、随机 IV 导致同文不同密、篡改任一字节后 MAC 校验失败、空串处理。

### 5.2 改造 `src-tauri/src/config.rs`

仅动四个出入口，函数签名除 `load_*` 改为返回 `Result<Option<(AppConfig, bool)>, String>` 外不变：

- `save_to_store`：`serde_json::to_string(config)` → `crypto::encrypt_str` → 以 **字符串** 值写回 `store.set(STORE_KEY, Value::String(env))`。
- `load_from_store`：三态返回——`Ok(Some((config, was_plaintext)))` 读取成功 / `Ok(None)` 无数据 / `Err` 有数据但解不开；
  - 若为 `Value::String(s)` 且 `crypto::is_encrypted(s)` → `decrypt_str` 后 `serde_json::from_str::<AppConfig>`；解密失败返回 `Err`（**绝不** 当作“无数据”而覆盖）；
  - 否则视为历史明文（`Value::Object` 或普通字符串）→ 按原逻辑解析，并标记 `was_plaintext = true`。
- `save_to_file`：写 `crypto::encrypt_str(&json)` 的信封文本。
- `load_from_file`：逐候选读取 → 密文走 `decrypt_str`，否则按明文解析并标记；若某候选**文件存在却读不出**（权限/非 UTF-8）、**密文解不开**、或**明文 JSON 解析失败**，均记录「不可读」后跳过，全部候选都读不出时返回 `Err`（**绝不** 当作“无数据”）。
- `load_config`：命中 `was_plaintext` 时，加载成功后立即回写密文（`save_to_store`/`save_to_file`），完成老数据升级；若 `load_*` 返回 `Err`（有数据但读不出，如钥匙串暂时不可用），则**只返回内存缓存或默认值、绝不回写**，避免把无法解密的既有配置覆盖掉。
- 其余（`migrate_config`、`mark_default_group`、`migrate_default_exclude`、`MEMORY_CACHE`）保持不变。

### 5.3 新增前端可用的 Tauri 命令

- 在 `src-tauri/src/commands.rs` 增加：
  - `vault_encrypt(plain: String) -> Result<String, String>`
  - `vault_decrypt(cipher: String) -> Result<String, String>`
- 在 `src-tauri/src/lib.rs` 顶部加 `mod crypto;`，并在 `generate_handler!` 中注册上述两个命令。
- **不引入 `sm-crypto`**：前端不再自行实现国密，统一复用 Rust 主密钥与算法（见第 6 节）。

### 5.4 VPN 密码路径改造（统一走 SM4）

现状：`vpn_encrypt_password`（`vpn/commands.rs:244`）调 `credential::protect` 返回 DPAPI/钥匙串密文，前端存入 `VpnProfile.password`；连接时 `vpn/commands.rs:397` 调 `credential::unprotect` 取回明文。

改为由 SM4 信封统一保护：

- 前端不再调 `vpn_encrypt_password`，直接把明文写入 `profile.password`，`password_encrypted` 置 `false`。
- 连接路径去掉 `credential::unprotect`，直接使用 `profile.password`；但先校验 `password_encrypted`：若仍为 `true`（说明旧密文迁移失败），拒绝连接并提示「旧密码尚未完成迁移，请重新填写」，避免把 `keychain:`/`dpapi:` 密文当明文发出去。
- 落盘保护由第 5.2 节的整块 SM4 信封承担（`password` 只是 `AppConfig` 里的一个字段）。
- `credential::protect/unprotect` 及其 DPAPI/钥匙串实现**保留**（主密钥托管仍复用它），只是不再用于 VPN 密码字段。
- 前端 VPN 表单里对 `vpn_encrypt_password` 的调用一并移除。

> 说明：此处不构成安全降级——主密钥本身仍托管在系统钥匙串 / DPAPI 中、按用户隔离；去掉的是字段级冗余加密，加密路径收敛为一条。

## 6. 前端改造

- `src/composables/useUpdater.ts`：把 `DISMISS_KEY` 的读写改为经 `invoke('vault_encrypt' | 'vault_decrypt')`。
  - 读取：`localStorage` 值为空 → `null`；为历史明文（非 `v1.`）→ 直接采用并回写密文；为密文 → `vault_decrypt`。前缀判定用 `trimStart()` 对齐后端 `is_encrypted` 的前导空白处理；`localStorage.getItem` 在隐私模式下抛 `SecurityError` 时按「无值」返回 `null`，避免未处理的 Promise rejection。
  - 写入：`vault_encrypt` 失败或未就绪时，降级为直接写明文，并打日志（不阻断更新提示功能）。
- 存储层抽一个极小的 `safeStorage` 封装（`getItem/setItem`），供后续 SP3 笔记复用。

## 7. 迁移与兼容

1. **历史明文配置**：首读即识别并回写密文，用户无感。
2. **历史 VPN 密码**：`password_encrypted == true` 且 `password` 为 `keychain:`/`dpapi:` 形式时，加载后 `credential::unprotect` 解出明文、写回 `password`、置 `password_encrypted = false`，随后回写密文块完成升级。解密失败（如换机后钥匙串条目不在）则保留原值并记日志，不阻断启动；此时 `password_encrypted` 仍为 `true`，连接会被 5.4 的守卫拦下并要求重新填写密码。（遗留：迁移成功后旧 macOS 钥匙串条目未清理，`credential::forget` 暂无调用点；删除须在落盘成功后进行，另行处理。）
3. **解密失败**（换机、钥匙串被清、密文损坏）：
   - 记录明确错误日志；
   - **不静默清空**：Store 读失败时继续走文件回退；全部失败才退回 `MEMORY_CACHE` → `AppConfig::default()`，行为与现有兜底一致；
   - 失败路径需在日志中给出「主密钥可能已丢失」的提示与恢复建议。
4. 回退副本（`config.json` 文件）与 Store 之间不交叉解密，各自独立处理。

## 8. 验收标准

- [ ] 打开 `<app_data_dir>/config.json`（及文件回退副本）为 `v1.` 信封密文，肉眼不可读。
- [ ] 界面内所有功能（项目分组、VPN 配置与规则、应用路径）操作正常，展示为明文。
- [ ] 老明文配置首次启动后被自动重写为密文。
- [ ] macOS 钥匙串中存在 `ws-tools-vault` / `master` 条目；重启后仍能正确解密。
- [ ] 手工篡改密文任一字节后应用不崩溃，日志报「数据已被篡改」并走兜底。
- [ ] 更新提示关闭标记在 `localStorage` 中为密文。
- [ ] 旧版 `keychain:`/`dpapi:` 密码的 VPN 配置升级后可正常连接，`config.json` 中不再出现这两个前缀。
- [ ] `cargo test` 全绿（含 `crypto` 模块新增单测）。
- [ ] `pnpm build`（`vue-tsc --noEmit && vite build`）通过。

## 9. 项目规范

在 `.trae/rules/project_rules.md` 追加：

> **本地存储一律加密**：凡写入本地磁盘的数据（配置文件、`localStorage` 等）必须密文存储，展示时解密；新增持久化字段须经 `crypto::encrypt_str` / `crypto::decrypt_str`（Rust）或 `safeStorage` 封装（前端），不得直接明文落盘。

## 10. 风险与已知问题

- **RustCrypto 依赖体积**：新增 6 个纯 Rust crate，编译时间与产物体积略有增加。
- **主密钥丢失即数据不可解**：属预期取舍，第 7 节给出降级路径。
- **VPN 密码处理**：按「统一走 SM4」的决策，`vpn_profiles[].password` 不再走 DPAPI/钥匙串字段级加密，而是明文存放于 `AppConfig` 内、随整块配置由 SM4 信封加密（第 5.4、7 节）。加密路径收敛为一条；由于主密钥本身仍按用户隔离托管，安全性不降低。
- **既有缺陷（本 spec 不修，仅记录）**：`config.rs::get_config_path` 为探测可写性，会调用 `save_to_store(app, &AppConfig::default())`，存在覆盖用户数据的副作用。与本次加密改造无关，另行处理。

## 11. 实施顺序

1. `Cargo.toml` 加依赖 → `crypto.rs`（含单测，`cargo test`）→ 主密钥托管。
2. `config.rs` 四出入口接入信封 + 历史明文升级。
3. VPN 密码路径改造：移除 `vpn_encrypt_password` 调用、连接路径去掉 `unprotect`、历史 `keychain:`/`dpapi:` 密文迁移为明文（第 5.4、7 节）。
4. `commands.rs` 新增 `vault_encrypt`/`vault_decrypt`，在 `lib.rs` 注册（并移除 `vpn_encrypt_password` 注册）。
5. 前端 `safeStorage` 封装 + `useUpdater.ts` 改造 + VPN 表单去掉加密调用。
6. 写项目规范。
7. 重启应用，按第 8 节验收。
