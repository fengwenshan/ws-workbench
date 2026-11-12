<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import { Input, Modal, Select, SelectOption, Switch } from 'ant-design-vue'
import type { VpnApi } from '@/composables/useVpn'
import type { VpnPolicy, VpnRule, VpnRuleKind } from '@/types/vpn'
import { POLICY_LABELS, RETIRED_RULE_KINDS, RULE_KIND_HINTS } from '@/types/vpn'

const props = defineProps<{
  vpn: VpnApi
}>()

const emit = defineEmits<{
  close: []
}>()

const vpn = props.vpn

/**
 * 只保留路由注入模式（规则模式的实际实现）能生效的类型：
 * 域名类靠系统按域名解析 + 本地转发器注入隧道路由。
 * `DOMAIN-KEYWORD` / `PROCESS-NAME` / `DST-PORT` 没有数据面可用，都已从下拉里下架；
 * `IP-CIDR` 现在按最长前缀注入，直连例外用更具体的 DIRECT 规则表达。
 * 历史配置里若还留着已下线的规则，列表里会打「不生效」标记，删掉即可。
 *
 * 顺序按「最常用在前」：域名后缀放第一个并设为默认 —— Windows 的 NRPT 精确匹配
 * 只覆盖裸域名，绝大多数场景要的是「这个域连同所有子域」。
 */
const KIND_OPTIONS: { value: VpnRuleKind; label: string }[] = [
  { value: 'DOMAIN-SUFFIX', label: '域名后缀 (DOMAIN-SUFFIX)' },
  { value: 'DOMAIN', label: '完整域名 (DOMAIN)' },
  { value: 'IP-CIDR', label: 'IP / 网段 (IP-CIDR)' },
]

/** 历史规则里可能还存着这些类型：能显示、能删，但不会再生效 */
const RETIRED_KINDS = new Set<string>(RETIRED_RULE_KINDS)

function retired(rule: VpnRule): boolean {
  return RETIRED_KINDS.has(rule.kind)
}

const POLICIES: VpnPolicy[] = ['DIRECT', 'VPN']

const profile = vpn.selectedProfile
/** 编辑期只改草稿；关闭弹窗时有改动才一次性落盘并热推送 */
const draftRules = ref<VpnRule[]>((profile?.rules ?? []).map((rule) => ({ ...rule })))
/** 没匹配到任何规则就走直连，不再提供兜底走向选项 */
const DEFAULT_POLICY: VpnPolicy = 'DIRECT'

const originalRules = JSON.stringify(profile?.rules ?? [])

const kind = ref<VpnRuleKind>('DOMAIN-SUFFIX')
const value = ref('')
const policy = ref<VpnPolicy>('VPN')
const error = ref('')
const adding = ref(false)
const saving = ref(false)
/** 正在编辑的规则 id；为 null 表示左侧表单处于「新增」状态 */
const editingId = ref<string | null>(null)
const editing = computed(() => editingId.value !== null)

const IPV4_OR_CIDR = /^\d{1,3}(?:\.\d{1,3}){3}(?:\/\d{1,2})?$/

/** 输入框占位随类型切换：IP 规则提示单 IP 与 CIDR 两种写法 */
const valuePlaceholder = computed(() =>
  kind.value === 'IP-CIDR'
    ? '192.168.6.115 或 192.168.6.0/24'
    : 'example.com（子域用 DOMAIN-SUFFIX）',
)

/** 域名规则里填了 IP：不阻止保存，只提示改用 IP 类型（后端会把它当域名，永远不命中） */
const ipWrittenAsDomain = computed(
  () => kind.value !== 'IP-CIDR' && IPV4_OR_CIDR.test(value.value.trim()),
)

const rules = computed(() => draftRules.value)
/** 列表头显示「N 条 · M 条启用」 */
const enabledCount = computed(() => rules.value.filter((rule) => rule.enabled).length)

/** 有改动才写盘：只是打开看一眼再关掉，不应该产生任何写入 */
const dirty = computed(() => JSON.stringify(draftRules.value) !== originalRules)

const valueInput = ref<{ focus: () => void } | null>(null)

/** 把右侧某一行的内容回填到左侧表单，进入编辑态 */
function startEdit(rule: VpnRule) {
  if (saving.value || adding.value || retired(rule)) return
  editingId.value = rule.id
  kind.value = rule.kind
  value.value = rule.value
  policy.value = rule.policy
  error.value = ''
  void nextTick(() => valueInput.value?.focus())
}

/** 放弃编辑：清空表单回到新增态，不碰右侧列表 */
function cancelEdit() {
  editingId.value = null
  value.value = ''
  error.value = ''
  void nextTick(() => valueInput.value?.focus())
}

/** 新增或保存修改：先本地挡重复，再让后端校验格式 */
async function submitRule() {
  if (adding.value || saving.value) return
  const text = value.value.trim()
  if (!text) {
    error.value = '规则内容不能为空'
    return
  }
  if (hasDuplicate(text)) {
    error.value = '已有相同规则'
    return
  }
  adding.value = true
  try {
    // 后端校验失败会抛错；域名规则原样返回
    const normalized = await vpn.validateRule(kind.value, text)
    const finalValue = (normalized || text).trim()
    if (hasDuplicate(finalValue)) {
      error.value = '已有相同规则'
      return
    }
    error.value = ''
    if (editingId.value) {
      const target = draftRules.value.find((rule) => rule.id === editingId.value)
      if (target) {
        target.kind = kind.value
        target.value = finalValue
        target.policy = policy.value
        // 表单里不再有启用开关：改一条被禁用的规则，保存后它就是启用的
        target.enabled = true
      }
      editingId.value = null
    } else {
      draftRules.value.push({
        id: crypto.randomUUID(),
        kind: kind.value,
        value: finalValue,
        policy: policy.value,
        remark: '',
        enabled: true,
      })
    }
    value.value = ''
    await nextTick()
    valueInput.value?.focus()
  } catch (err: any) {
    error.value = String(err ?? '规则格式不正确')
  } finally {
    adding.value = false
  }
}

/** 同类型 + 同内容 + 同策略算重复；编辑时忽略自己 */
function hasDuplicate(text: string): boolean {
  return rules.value.some(
    (rule) =>
      rule.id !== editingId.value &&
      rule.kind === kind.value &&
      rule.value === text &&
      rule.policy === policy.value,
  )
}

function onKindChange(next: VpnRuleKind) {
  kind.value = next
  error.value = ''
}

function onValueInput() {
  error.value = ''
}

function toggleRule(ruleId: string, enabled: boolean) {
  const target = draftRules.value.find((rule) => rule.id === ruleId)
  if (target) target.enabled = enabled
}

function removeRule(ruleId: string) {
  if (editingId.value === ruleId) cancelEdit()
  draftRules.value = draftRules.value.filter((rule) => rule.id !== ruleId)
}

function moveRule(ruleId: string, delta: number) {
  const list = [...draftRules.value]
  const index = list.findIndex((rule) => rule.id === ruleId)
  const target = index + delta
  if (index < 0 || target < 0 || target >= list.length) return
  const [moved] = list.splice(index, 1)
  list.splice(target, 0, moved)
  draftRules.value = list
}

/** 关闭（完成 / × / 点遮罩都走这里）：有改动才保存并热推送一次 */
async function closeDialog() {
  if (saving.value) return
  if (dirty.value) {
    saving.value = true
    const startedAt = Date.now()
    try {
      await vpn.saveRules(draftRules.value, DEFAULT_POLICY)
    } catch (err: any) {
      // 保存/下发失败就留在弹窗里，别一关遮住错误；用户还能改完重试
      vpn.pushLog('error', `保存规则失败: ${err}`)
      return
    } finally {
      // 未连接时写盘很快，避免 loading 只闪一帧、看起来仍像没反应
      const remaining = 300 - (Date.now() - startedAt)
      if (remaining > 0) {
        await new Promise((resolve) => window.setTimeout(resolve, remaining))
      }
      saving.value = false
    }
  }
  emit('close')
}
</script>

<template>
  <Modal
    :open="true"
    title="编辑规则"
    :width="780"
    :footer="null"
    :closable="true"
    :mask-closable="true"
    centered
    wrap-class-name="vpn-rule-modal"
    @cancel="closeDialog"
  >
    <div class="rule-editor" :aria-busy="saving">
      <!-- 左：三个输入项 + 添加 -->
      <div class="rule-form">
        <div class="form-field">
          <label>规则类型</label>
          <Select
            :value="kind"
            class="full-width"
            @change="(next: any) => onKindChange(next as VpnRuleKind)"
          >
            <SelectOption v-for="option in KIND_OPTIONS" :key="option.value" :value="option.value">
              {{ option.label }}
            </SelectOption>
          </Select>
        </div>

        <div class="form-field">
          <label>规则内容</label>
          <Input
            ref="valueInput"
            v-model:value="value"
            :placeholder="valuePlaceholder"
            :status="error ? 'error' : undefined"
            spellcheck="false"
            @input="onValueInput"
            @press-enter="submitRule"
          />
          <p v-if="ipWrittenAsDomain" class="field-hint">
            这看起来是 IP：域名规则填 IP 不会生效，请把类型改成「IP / 网段」
          </p>
          <p v-if="error" class="field-error">{{ error }}</p>
        </div>

        <div class="form-field">
          <label>代理策略</label>
          <Select v-model:value="policy" class="full-width">
            <SelectOption v-for="item in POLICIES" :key="item" :value="item">
              {{ POLICY_LABELS[item] }}
            </SelectOption>
          </Select>
        </div>

        <div class="form-actions">
          <button
            class="btn btn-primary add-btn"
            :disabled="adding || saving"
            @click="submitRule"
          >
            {{ adding ? (editing ? '保存中…' : '添加中…') : editing ? '保存修改' : '添加规则' }}
          </button>
          <button
            v-if="editing"
            class="btn btn-secondary"
            type="button"
            :disabled="adding"
            @click="cancelEdit"
          >
            取消
          </button>
        </div>
      </div>

      <!-- 右：现有规则 -->
      <div class="rule-list-wrap">
        <div class="rule-list-head">
          <span>规则列表</span>
          <span class="rule-list-count">{{ rules.length }} 条 · {{ enabledCount }} 条启用</span>
        </div>

        <div class="rule-list">
          <div v-if="rules.length === 0" class="rule-list-empty">
            还没有规则。严格规则模式下只有这里命中的域名才会走 VPN，服务端下发的内网范围不会自动生效；「直连」规则可以当例外用（更具体的规则优先）。在左边填好类型、内容和策略，点「添加规则」。
          </div>

          <div
            v-for="(rule, index) in rules"
            :key="rule.id"
            class="rule-row"
            :class="{ disabled: !rule.enabled, editing: rule.id === editingId }"
            :title="retired(rule) ? '历史类型不能编辑，建议删掉重建' : '双击编辑这条规则'"
            @dblclick="startEdit(rule)"
          >
            <span class="row-index">{{ index + 1 }}</span>
            <div class="row-main">
              <span class="row-value">{{ rule.value }}</span>
              <div class="row-meta">
                <span class="row-kind">{{ RULE_KIND_HINTS[rule.kind] ?? rule.kind }}</span>
                <span class="row-policy" :class="rule.policy.toLowerCase()">
                  {{ POLICY_LABELS[rule.policy] }}
                </span>
                <span
                  v-if="retired(rule)"
                  class="row-retired"
                  title="这个类型在路由注入模式下做不到，不会生效：域名请改用 DOMAIN-SUFFIX"
                >
                  不生效
                </span>
              </div>
            </div>
            <div class="row-actions" @dblclick.stop>
              <button
                class="icon-btn"
                type="button"
                title="编辑"
                aria-label="编辑"
                :disabled="retired(rule) || saving"
                @click="startEdit(rule)"
              >
                <svg viewBox="64 64 896 896" aria-hidden="true">
                  <path
                    d="M257.7 752c2 0 4-.2 6-.5L431.9 722c2-.4 3.9-1.3 5.3-2.8l423.9-423.9a9.96 9.96 0 0 0 0-14.1L694.9 114.6c-1.9-1.9-4.4-2.9-7.1-2.9s-5.2 1-7.1 2.9L256.8 538.8c-1.5 1.5-2.4 3.3-2.8 5.3l-29.5 168.2a33.5 33.5 0 0 0 9.4 29.8c6.6 6.4 14.9 9.9 23.8 9.9zm67.4-174.4L687.8 215l73.3 73.3-362.7 362.6-88.9 15.7 15.6-89zM880 836H144c-17.7 0-32 14.3-32 32v36c0 4.4 3.6 8 8 8h784c4.4 0 8-3.6 8-8v-36c0-17.7-14.3-32-32-32z"
                  />
                </svg>
              </button>
              <button
                class="icon-btn"
                type="button"
                title="上移"
                aria-label="上移"
                :disabled="index === 0"
                @click="moveRule(rule.id, -1)"
              >
                <svg viewBox="64 64 896 896" aria-hidden="true">
                  <path d="M512 96 784 352H576V736H448V352H240Z" />
                </svg>
              </button>
              <button
                class="icon-btn"
                type="button"
                title="下移"
                aria-label="下移"
                :disabled="index === rules.length - 1"
                @click="moveRule(rule.id, 1)"
              >
                <svg viewBox="64 64 896 896" aria-hidden="true">
                  <path d="M512 928 240 672H448V288H576V672H784Z" />
                </svg>
              </button>
              <button
                class="icon-btn danger"
                type="button"
                title="删除"
                aria-label="删除"
                @click="removeRule(rule.id)"
              >
                <svg viewBox="64 64 896 896" aria-hidden="true">
                  <path
                    d="M799.86 166.31c.02 0 .04.02.08.06l57.69 57.7c.04.03.05.05.06.08a.12.12 0 010 .06c0 .03-.02.05-.06.09L569.93 512l287.7 287.7c.04.04.05.06.06.09a.12.12 0 010 .07c0 .02-.02.04-.06.08l-57.7 57.69c-.03.04-.05.05-.07.06a.12.12 0 01-.07 0c-.03 0-.05-.02-.09-.06L512 569.93l-287.7 287.7c-.04.04-.06.05-.09.06a.12.12 0 01-.07 0c-.02 0-.04-.02-.08-.06l-57.69-57.7c-.04-.03-.05-.05-.06-.07a.12.12 0 010-.07c0-.03.02-.05.06-.09L454.07 512l-287.7-287.7c-.04-.04-.05-.06-.06-.09a.12.12 0 010-.07c0-.02.02-.04.06-.08l57.7-57.69c.03-.04.05-.05.07-.06a.12.12 0 01.07 0c.03 0 .05.02.09.06L512 454.07l287.7-287.7c.04-.04.06-.05.09-.06a.12.12 0 01.07 0z"
                  />
                </svg>
              </button>
            </div>
            <span
              class="row-switch"
              :title="rule.enabled ? '已启用，点击禁用（不删除规则）' : '已禁用，点击启用'"
            >
              <Switch
                size="small"
                :checked="rule.enabled"
                @dblclick.stop
                @change="(checked: any) => toggleRule(rule.id, !!checked)"
              />
            </span>
          </div>
        </div>
      </div>

      <div v-if="saving" class="rule-saving-mask" role="status" aria-live="polite">
        <span class="btn-spinner" aria-hidden="true" />
        正在保存并应用规则…
      </div>
    </div>

    <div class="rule-editor-footer">
      <button
        class="btn btn-primary"
        :disabled="saving"
        :aria-busy="saving"
        @click="closeDialog"
      >
        <span v-if="saving" class="btn-spinner" aria-hidden="true" />
        {{ saving ? '保存中…' : '完成' }}
      </button>
    </div>
  </Modal>
</template>

<style scoped>
.rule-editor {
  position: relative;
  display: flex;
  gap: 16px;
  min-height: 360px;
}
.rule-saving-mask {
  position: absolute;
  inset: 0;
  z-index: 5;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  border-radius: 8px;
  background: color-mix(in srgb, var(--bg-primary) 82%, transparent);
  color: var(--text-secondary);
  font-size: 14px;
  backdrop-filter: blur(1px);
}
.btn-spinner {
  display: inline-block;
  flex: none;
  width: 14px;
  height: 14px;
  border: 2px solid currentColor;
  border-right-color: transparent;
  border-radius: 50%;
  animation: ws-rule-spin 0.7s linear infinite;
  vertical-align: -2px;
}
@keyframes ws-rule-spin {
  to {
    transform: rotate(360deg);
  }
}
.rule-form {
  width: 280px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.form-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.form-field label {
  font-size: 12px;
  color: var(--text-secondary);
}
.full-width {
  width: 100%;
}
.field-error {
  font-size: 12px;
  color: var(--danger);
  line-height: 1.4;
}
.field-hint {
  margin-top: 4px;
  font-size: 12px;
  color: var(--text-muted);
}
.form-actions {
  display: flex;
  gap: 8px;
  margin-top: 4px;
}
.add-btn {
  flex: 1;
  min-width: 0;
}
.rule-list-wrap {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  overflow: hidden;
}
.rule-list-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  font-size: 12px;
  color: var(--text-secondary);
  border-bottom: 1px solid var(--border-color);
  background: var(--bg-secondary);
}
.rule-list-count {
  color: var(--text-muted);
}
.rule-list {
  flex: 1;
  overflow-y: auto;
  max-height: 380px;
  padding: 6px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.rule-list-empty {
  padding: 24px 12px;
  font-size: 12px;
  color: var(--text-muted);
  text-align: center;
  line-height: 1.7;
}
.rule-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border-radius: 6px;
  border: 1px solid transparent;
  font-size: 14px;
}
.rule-row:hover {
  background: var(--bg-hover);
}
.rule-row.disabled .row-value {
  color: var(--text-muted);
  text-decoration: line-through;
}
.rule-row.editing {
  border-color: var(--primary);
  background: color-mix(in srgb, var(--primary) 7%, transparent);
}
.rule-row.editing:hover {
  background: color-mix(in srgb, var(--primary) 11%, transparent);
}
.row-index {
  width: 18px;
  flex-shrink: 0;
  text-align: right;
  font-size: 12px;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}
.row-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.row-value {
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.row-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.row-kind {
  flex-shrink: 0;
  font-size: 10px;
  color: var(--text-muted);
  border: 1px solid var(--border-color);
  border-radius: 4px;
  padding: 0 4px;
  white-space: nowrap;
}
.row-retired {
  flex-shrink: 0;
  font-size: 10px;
  color: #b45309;
  background: rgba(245, 158, 11, 0.12);
  border-radius: 4px;
  padding: 1px 6px;
  white-space: nowrap;
}
.row-switch {
  display: inline-flex;
  flex-shrink: 0;
}
.row-policy {
  flex-shrink: 0;
  font-size: 10px;
  border-radius: 4px;
  padding: 1px 6px;
}
.row-policy.vpn {
  color: #2563eb;
  background: rgba(59, 130, 246, 0.1);
}
.row-policy.direct {
  color: var(--text-secondary);
  background: var(--bg-secondary);
}
.row-actions {
  display: flex;
  gap: 2px;
  flex-shrink: 0;
  opacity: 0.45;
  transition: opacity 0.12s;
}
.rule-row:hover .row-actions {
  opacity: 1;
}
.icon-btn {
  display: grid;
  place-items: center;
  width: 20px;
  height: 20px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1;
  cursor: pointer;
  padding: 0;
}
.icon-btn svg {
  width: 12px;
  height: 12px;
  fill: currentColor;
}
.icon-btn:hover:not(:disabled) {
  border-color: var(--primary);
  color: var(--primary);
}
.icon-btn:disabled {
  opacity: 0.35;
  cursor: default;
}
.icon-btn.danger:hover:not(:disabled) {
  border-color: var(--danger);
  color: var(--danger);
}
.rule-editor-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  margin-top: 16px;
  padding-top: 12px;
  border-top: 1px solid var(--border-color);
}
.rule-editor-footer .btn-spinner {
  margin-right: 6px;
}
</style>
