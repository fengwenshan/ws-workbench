# layui 项目打包规则：排除（黑名单）→ 白名单

日期：2026-11-04
状态：已实现

## 背景

原来的打包规则是「排除规则」：默认规则 + 项目级规则合并成 `ignore` 的 `!pattern` 黑名单，凑齐一长串 `.git`、`.idea`、`*.md`、`*.scss`… 才能保证包里只剩浏览器资源。想加类型就得继续往黑名单里堆，越堆越难维护，也容易漏（漏一个 `.thm` 之类就把不该上传的文件带上去了）。

## 决定

1. **改成包含规则（白名单）**：只有命中规则的文件才进包。默认规则只列浏览器能直接加载/渲染的类型。
2. **`.` / `_` 开头的路径段一律跳过**，任意层级、文件或目录都算，写死在遍历里，不占规则名额。
3. **规则只有一份，放全局设置页**（设置 →「layui 项目打包规则」）；项目级排除规则整块删除，`ProjectConfig.exclude` 字段移除。
4. 规则清空 = 视为未配置：除 `.`/`_` 开头与含空格路径外全部打包（避免误删规则后打出空包）。

## 语义

- 不带 `/` 的规则按**文件名**匹配、任意层级都算：`*.png`、`index.html`
- 带 `/` 的规则按**相对源目录的路径**匹配：`assets/**`、`js/app.js`
- 规则命中其一即包含；白名单为空视为未配置
- 路径段以 `.` 或 `_` 开头：硬性剔除，规则无法放行
- 相对路径任一段含空格：跳过（原有行为，保留）
- 输出目录 / zip 自身若在源目录内：整棵子树剪掉（`extra_excludes`，原有行为）

默认包含集（`DEFAULT_INCLUDE`）：`html htm css js mjs json png jpg jpeg gif svg webp avif ico bmp woff woff2 ttf eot otf mp4 webm mp3 wav ogg txt xml thm tjs dotm docx`。依据是 `demo-oa-layui` 的实际后缀与引用分析：常规资源之外，layui 工程的 `.thm`（`rsViewRender.js` 运行时拉取的模板）、`.tjs`（模板脚本）、`.dotm`（WPS 控件 `Normal.dotm`）、`.docx`（`rsWebOffice.js` 通过 `$.ajax` GET 的 `weboffice/newDocument.docx` 新建正文模板）也必须进包；`scss`/`map`/`md`/`pdf` 等只是开发期或上传字符串，不进包。

## 架构

- `models.rs`：`DEFAULT_INCLUDE`、`DEFAULT_INCLUDE_VERSION = 1`、`default_include_rules()`、`migrate_default_include()`；`AppSettings`/`AppConfig` 字段 `default_exclude` → `default_include`（旧 JSON 里的 `default_exclude` 由 serde 忽略，版本 0 触发迁移补齐默认集）
- `packer.rs`：`scan_files(source, includes, extra_excludes)` 三层过滤 —— `filter_entry` 剪掉 `.`/`_`/输出目录/zip 自身，`globset` 做文件名与路径两套白名单匹配，空格路径跳过
- `commands/project.rs`：`pack_project` / `pack_to_zip` 参数改名 `default_include`
- `commands/config.rs`：`get_default_exclude_rules` → `get_default_include_rules`
- 前端：`usePack` 传 `defaultInclude`；设置页新增「layui 项目打包规则」卡片（`RulesEditor` 组件 + 恢复默认）；项目配置页删掉「项目排除规则」「默认排除规则」两块

## 错误处理

- 规则含非法 glob：打包时报「包含规则 'x' 解析失败」，不静默吞掉
- 规则为空：全部放行，日志正常
- 扫描结果为空：沿用原有 `未找到可复制的文件` 错误提示

## 测试

- `models::default_include_tests`：默认集覆盖浏览器类型、不含开发期类型、无重复、老配置迁移补齐、版本已最新时不回塞
- `packer::tests`：`.`/`_` 硬跳过（含子目录）、默认集只收浏览器资源、文件名/路径两种匹配、空规则放行、输出目录剪枝
- 前端 `vue-tsc --noEmit` + `vite build`

## 后续变更

- 2026-11-04：默认包含集加入 `*.doc`（旧版 Word 模板，部分 layui 工程的 WebOffice 新建正文只提供 `.doc`），`DEFAULT_INCLUDE_VERSION` 升到 4；老配置加载时由 `migrate_default_include` 自动追加该规则。
- 2026-11-12：默认包含集加入 `*.cab` 与 `*.ftl`（NTKO 正文控件：`rsOfficeRender.js` 的 `codebase` 指向 `lib/ntko/ntkoControl.cab`，控件模板 `ntko_lib.ftl` 由后端 include），`DEFAULT_INCLUDE_VERSION` 升到 5；老配置加载时由 `migrate_default_include` 自动追加这两条规则。
- 2026-11-14：打包规则从设置页移到项目「配置 → 打包包含规则」，每个 layui 项目一套（`ProjectConfig.pack_include: Option<Vec<String>>`；`None` = 继承全局默认 `AppConfig.default_include`，也就是老设置页那份规则）。输入纯后缀（`psd` / `.psd`）自动补成 `*.psd`；清空规则时回退到继承默认，避免空数组被后端当成「全部放行」。前端打包时取「项目规则非空 ? 项目规则 : 全局默认」，不把空数组传给打包器。
