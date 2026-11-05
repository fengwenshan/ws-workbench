# 分流规则改为 Clash 式弹窗编辑

日期：2026-11-05
状态：待评审

## 背景

现在的分流规则是面板里一张内嵌表格，列有 类型 / 匹配值 / 走向 / no-resolve / 启用 / 备注 / 操作。对只想"给某个域名指定走 VPN 还是直连"的用户来说，信息过密：no-resolve、备注这些选项既看不懂，也很少改。

改成 Clash Verge 那种方式：面板只做只读预览，增删改序都在一个「编辑规则」弹窗里完成，弹窗只有三个输入项。

## 目标

1. 面板卡片：标题 + 说明 + 只读规则列表 + 右上角「编辑规则」按钮
2. 弹窗：左边三个输入项（规则类型、规则内容、代理策略）+ 右边现有规则列表
3. 规则顺序：只有一个「添加规则」按钮（追加到末尾），每条规则 hover 出 ↑↓ 调整顺序
4. 兜底 MATCH 走向从表格里挪到弹窗列表末尾的固定行
5. 启用/停用保留为行内小开关

## 不在范围

- 实时连接日志 / 拦截日志（另一个需求，稍后单独做）
- REJECT 拦截策略（规则走向仍为 DIRECT / VPN）
- 规则分组、规则集、订阅

## 界面

面板卡片：

```
分流规则                                   [编辑规则]
从上到下匹配，命中第一条即停止；都不命中时走 MATCH 走向。

  example.com        DOMAIN-KEYWORD   DIRECT
  corp.example.com   DOMAIN-SUFFIX    VPN        （备注作为副标题显示）
  ...
  MATCH  其余全部流量                      DIRECT
```

- 整行点击也打开弹窗（不预填表单，只是快捷入口）；空列表显示「还没有规则，点「编辑规则」添加」
- 列表只读，长列表限高滚动（约 6 行），超出滚动；停用的规则置灰加删除线，仍可见

弹窗（宽 ~760，左表单 300 + 右列表自适应）：

- 规则类型下拉：完整域名 DOMAIN / 域名后缀 DOMAIN-SUFFIX / 域名关键字 DOMAIN-KEYWORD / 进程名 PROCESS-NAME / 目标端口 DST-PORT / IP 网段 IP-CIDR
- 规则内容输入框，placeholder 跟着类型变：
  - DOMAIN → `example.com`
  - DOMAIN-SUFFIX → `example.com`（连带所有子域）
  - DOMAIN-KEYWORD → `google`
  - PROCESS-NAME → `Google Chrome`
  - DST-PORT → `443`
  - IP-CIDR → `10.0.0.0/8`
- 代理策略下拉：直连 DIRECT / 走 VPN
- 「添加规则」按钮：校验 → 追加到末尾 → 清空内容框，焦点回内容框
- 右侧规则行：序号 + 值（备注作副标题）+ 类型标签 + 策略 + 操作区（↑ ↓ 删除，hover 才显示）+ 启用开关
- 列表末尾固定 MATCH 行：`MATCH` + `其余全部流量` + 兜底走向下拉（不可删除）
- 底部只留「完成」（不改草稿，所有变更即时写配置）

## 行为

- 所有变更即时 `saveConfig()`；已连接且是当前生效配置时，沿用现有 300ms 防抖热推送
- 校验复用后端 `vpn_validate_rule`；内容为空或格式错误时输入框红框 + 下方红字，不写入
- 完全相同的「类型 + 值 + 策略」拒绝添加，提示"已有相同规则"（前端比较，不往返后端）
- 新规则默认 `enabled: true`、`no_resolve: true`（与引擎默认一致；该字段仅对 IP-CIDR 有意义，UI 不再暴露）
- 老数据里的 `remark` 只读展示为副标题，不再提供编辑入口（清空需求后续再说）

## 代码改动

- 新增 `src/components/vpn/VpnRuleDialog.vue`
- 删除 `src/components/vpn/VpnRuleTable.vue`（唯一使用方是 VpnPanel）
- `src/components/vpn/VpnPanel.vue`：规则卡片改成只读预览 + 弹窗开关状态
- `src/composables/useVpn.ts`：`addRule` 默认 `no_resolve: true`；新增 `moveRule(ruleId, delta)`（上下移动并触发推送）
- 类型与 Rust 侧不改（`remark` / `no_resolve` 字段保留在数据里）

## 验证

- `pnpm exec vue-tsc --noEmit`、`pnpm build`
- 手动：加规则 / 删规则 / 上下移 / 改兜底 / 停用启用 / 连接中编辑热生效 / 老配置带备注与 no-resolve 正常显示
