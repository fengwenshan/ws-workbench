# layui 项目打包规则：白名单 → 排除（黑名单）

日期：2026-11-19
状态：已实现
取代：[2026-11-04-pack-include-rules-design.md](2026-11-04-pack-include-rules-design.md) 的「包含规则」决定。

## 背景

原来的打包规则是白名单：只有命中规则的文件才进包（`DEFAULT_INCLUDE` 34 条，覆盖 html/css/js/图片/字体/模板/控件等）。维护一份「有用类型」清单在实际项目里容易漏：新增一种运行时资源就要改默认规则并升版本，老项目还要等迁移。

改成反向：列**不打包**的格式，其余全部打包。默认清单只留开发期产物与打包垃圾，新类型默认就会进包。

## 决定

1. **规则语义改成排除（黑名单）**：命中任一规则的文件不进包，其余全部打包。
2. **隐藏项仍由 `.*` 兜住**；`_*` 不再默认排除，`_` 开头的文件/目录按普通文件打包，需要时自己加规则。
3. **默认排除集**（`DEFAULT_EXCLUDE`，版本 4）：
   - `.*`：隐藏文件/目录（`.git`、`.svn`、`.idea`、`.vscode`、`.DS_Store`…）
   - `docs/**`：文档目录
   - `*.md`、`*.scss`、`*.less`：文档与样式源码
   - `*.map`：sourcemap
   - `*.test.js`、`*.spec.js`、`__tests__`：测试文件与测试目录（目录名规则任意层级都剪）
   - `node_modules`：依赖目录，整棵剪掉
   - `*.zip`：历史打包产物
4. **字段改名**：`ProjectConfig.pack_include` → `pack_exclude`，`AppConfig/AppSettings.default_include(_version)` → `default_exclude(_version)`。旧白名单字段直接忽略（白名单无法等价翻译成黑名单），升级后项目回落为继承新的默认排除规则。
5. **空规则 = 不排除**（全部打包）。前端把项目规则清空时归一成 `None` 回退到全局默认，避免把 `.git`、`node_modules` 打进包。
6. **含空格的路径拆成两个项目级开关**：`pack_skip_spaced_dirs` / `pack_skip_spaced_files`，默认都为 `true`；取消勾选后含空格的目录 / 文件照常打包。

## 语义（与 .gitignore 一致）

- 不带 `/` 的规则匹配任意层级的名字（`*.md`、`.git`、`node_modules`）
- 带 `/` 的规则从源目录锚定（`/dist/`、`assets/**`、`js/app.js`）
- 结尾 `/` 只匹配目录（`docs/`）
- `**` 跨层级（`**/__tests__/**`）
- `!` 表示重新包含（`!README.md`）；`#` 开头是注释
- 被排除的目录整棵剪掉，目录里的文件不能用 `!` 再包含回来（gitignore 同样限制）
- 含空格的目录 / 文件：按项目开关跳过，默认都跳过；关掉后照常打包。源目录自身含空格不受影响
- 输出目录 / zip 自身若在源目录内：整棵子树剪掉（`extra_excludes`，原有行为）

## 架构

- `models.rs`：`DEFAULT_EXCLUDE`、`DEFAULT_EXCLUDE_ADDITIONS`、`DEFAULT_EXCLUDE_REMOVALS`、`DEFAULT_EXCLUDE_VERSION = 4`、`default_exclude_rules()`、`migrate_default_exclude()`；`AppSettings` / `AppConfig` 字段改名
- `packer/mod.rs`：`build_exclude_matcher` 用 `ignore::gitignore::GitignoreBuilder` 编译规则；`scan_files` 在 `filter_entry` 里按 gitignore 匹配剪目录，文件用同一匹配器过滤，再按项目开关跳过含空格的目录 / 文件
- `commands/project.rs`：`pack_project` / `pack_to_zip` 参数改为 `default_exclude`
- `commands/config.rs` / `lib.rs`：`get_default_exclude_rules` 命令改名
- 前端：`pack_exclude` / `default_exclude` 字段、`usePack` 传 `defaultExclude`、项目配置页标题改成「打包排除规则」

## 错误处理

- 规则含非法 glob：打包时报「排除规则 'x' 解析失败」，不静默吞掉
- 规则为空：不排除，正常打包
- 扫描结果为空：沿用原有「未找到可复制的文件」错误提示

## 测试

- `models::default_exclude_tests`：默认集覆盖隐藏目录、文档目录、样式源码、测试、依赖与压缩包、无重复、老配置迁移补齐、版本已最新时不回塞、空间过滤字段缺省为 true
- `packer::tests`：默认规则只挡清单内文件、`.`/`_` 条目由默认规则排除、gitignore 语义（`!` / 锚定 / 目录结尾 `/` / `**`）、空规则全部打包、含空格目录 / 文件两个开关互相独立、输出目录剪枝
- 前端 `vue-tsc --noEmit` + `vite build`

## 版本迁移

- 版本 0（没有 exclude 字段的老配置）：补齐完整默认集
- 版本 1 → 2：只追加测试相关规则（`*.test.js`、`*.spec.js`、`__tests__`），不复活用户在前一版里删掉的规则
- 版本 2 → 3：移除 `_*`，`_` 开头的文件/目录默认会进包
- 版本 3 → 4：追加 `docs/**`

## 已知取舍

黑名单不保底：清单外的开发文件（`*.ts`、`*.vue`、`package.json`、`*.psd`、`*.pdf`、`*.log`…）默认会进包。认为不该带的，在项目「配置 → 打包排除规则」里补一条即可，不再动默认规则和版本号。
