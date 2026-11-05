<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import { Input, Modal, Select, SelectOption, Switch } from 'ant-design-vue'
import type { VpnApi } from '@/composables/useVpn'
import type { VpnPolicy, VpnRuleKind } from '@/types/vpn'
import { POLICY_LABELS } from '@/types/vpn'

const props = defineProps<{
  vpn: VpnApi
}>()

const emit = defineEmits<{
  close: []
}>()

const vpn = props.vpn

/** 六个类型按「域名三种 → 其它」排，前三个覆盖绝大多数用法 */
const KIND_OPTIONS: { value: VpnRuleKind; label: string; placeholder: string }[] = [
  { value: 'DOMAIN', label: '完整域名 (DOMAIN)', placeholder: 'example.com' },
  { value: 'DOMAIN-SUFFIX', label: '域名后缀 (DOMAIN-SUFFIX)', placeholder: 'example.com（连带所有子域）' },
  { value: 'DOMAIN-KEYWORD', label: '域名关键字 (DOMAIN-KEYWORD)', placeholder: 'google' },
  { value: 'PROCESS-NAME', label: '进程名 (PROCESS-NAME)', placeholder: 'Google Chrome' },
  { value: 'DST-PORT', label: '目标端口 (DST-PORT)', placeholder: '443' },
  { value: 'IP-CIDR', label: 'IP 网段 (IP-CIDR)', placeholder: '10.0.0.0/8' },
]

const POLICIES: VpnPolicy[] = ['DIRECT', 'VPN']

const kind = ref<VpnRuleKind>('DOMAIN-SUFFIX')
const value = ref('')
const policy = ref<VpnPolicy>('VPN')
const error = ref('')
const adding = ref(false)

const rules = computed(() => vpn.selectedProfile?.rules ?? [])
const defaultPolicy = computed<VpnPolicy>(
  () => vpn.selectedProfile?.rules_default_policy ?? 'DIRECT',
)

const placeholder = computed(
  () => KIND_OPTIONS.find((option) => option.value === kind.value)?.placeholder ?? '',
)

const valueInput = ref<{ focus: () => void } | null>(null)

/** 添加：先本地挡重复，再让后端校验格式，最后追加到末尾 */
async function addRule() {
  if (adding.value) return
  const text = value.value.trim()
  if (!text) {
    error.value = '规则内容不能为空'
    return
  }
  const duplicate = rules.value.some(
    (rule) => rule.kind === kind.value && rule.value === text && rule.policy === policy.value,
  )
  if (duplicate) {
    error.value = '已有相同规则'
    return
  }
  adding.value = true
  try {
    const reason = await vpn.validateRule(kind.value, text)
    if (reason) {
      error.value = reason
      return
    }
    error.value = ''
    await vpn.addRule(kind.value, text, policy.value)
    value.value = ''
    await nextTick()
    valueInput.value?.focus()
  } catch (err: any) {
    error.value = `添加失败: ${err}`
  } finally {
    adding.value = false
  }
}

function onKindChange(next: VpnRuleKind) {
  kind.value = next
  error.value = ''
}

function onValueInput() {
  error.value = ''
}

async function toggleRule(ruleId: string, enabled: boolean) {
  await vpn.updateRule(ruleId, { enabled })
}

async function removeRule(ruleId: string) {
  await vpn.deleteRule(ruleId)
}

async function moveRule(ruleId: string, delta: number) {
  await vpn.moveRule(ruleId, delta)
}

async function setDefaultPolicy(next: VpnPolicy) {
  await vpn.updateRulesDefaultPolicy(next)
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
    @cancel="emit('close')"
  >
    <div class="rule-editor">
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
            :placeholder="placeholder"
            :status="error ? 'error' : undefined"
            spellcheck="false"
            @input="onValueInput"
            @press-enter="addRule"
          />
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

        <button class="btn btn-primary add-btn" :disabled="adding" @click="addRule">
          {{ adding ? '添加中…' : '添加规则' }}
        </button>
        <p class="form-note">新规则追加到末尾；规则从上往下匹配，第一条命中即生效。</p>
      </div>

      <!-- 右：现有规则 -->
      <div class="rule-list-wrap">
        <div class="rule-list-head">
          <span>规则列表</span>
          <span class="rule-list-count">{{ rules.length }} 条</span>
        </div>

        <div class="rule-list">
          <div v-if="rules.length === 0" class="rule-list-empty">
            还没有规则。左边填好类型、内容和策略，点「添加规则」。
          </div>

          <div
            v-for="(rule, index) in rules"
            :key="rule.id"
            class="rule-row"
            :class="{ disabled: !rule.enabled }"
          >
            <span class="row-index">{{ index + 1 }}</span>
            <div class="row-main">
              <span class="row-value">{{ rule.value }}</span>
              <div class="row-meta">
                <span class="row-kind">{{ rule.kind }}</span>
                <span class="row-policy" :class="rule.policy.toLowerCase()">
                  {{ POLICY_LABELS[rule.policy] }}
                </span>
                <span v-if="rule.remark" class="row-remark">{{ rule.remark }}</span>
              </div>
            </div>
            <div class="row-actions">
              <button
                class="icon-btn"
                type="button"
                title="上移"
                aria-label="上移"
                :disabled="index === 0"
                @click="moveRule(rule.id, -1)"
              >
                ↑
              </button>
              <button
                class="icon-btn"
                type="button"
                title="下移"
                aria-label="下移"
                :disabled="index === rules.length - 1"
                @click="moveRule(rule.id, 1)"
              >
                ↓
              </button>
              <button
                class="icon-btn danger"
                type="button"
                title="删除"
                aria-label="删除"
                @click="removeRule(rule.id)"
              >
                ×
              </button>
            </div>
            <Switch
              size="small"
              :checked="rule.enabled"
              @change="(checked: any) => toggleRule(rule.id, !!checked)"
            />
          </div>

          <!-- 兜底：规则全不命中时的走向，固定不可删 -->
          <div class="rule-row rule-row-match">
            <span class="row-index">—</span>
            <div class="row-main">
              <span class="row-value mono">MATCH</span>
              <div class="row-meta">
                <span class="row-kind">兜底</span>
                <span class="row-remark">其余全部流量</span>
              </div>
            </div>
            <div class="row-policy-select">
              <Select
                size="small"
                :value="defaultPolicy"
                @change="(next: any) => setDefaultPolicy(next as VpnPolicy)"
              >
                <SelectOption v-for="item in POLICIES" :key="item" :value="item">
                  {{ POLICY_LABELS[item] }}
                </SelectOption>
              </Select>
            </div>
            <span class="match-spacer"></span>
          </div>
        </div>
      </div>
    </div>

    <div class="rule-editor-footer">
      <span class="footer-hint">改动即时保存，已连接时自动生效</span>
      <button class="btn btn-primary" @click="emit('close')">完成</button>
    </div>
  </Modal>
</template>

<style scoped>
.rule-editor {
  display: flex;
  gap: 16px;
  min-height: 360px;
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
.add-btn {
  margin-top: 4px;
  width: 100%;
}
.form-note {
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.5;
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
  line-height: 1.6;
}
.rule-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border-radius: 6px;
  border: 1px solid transparent;
  font-size: 13px;
}
.rule-row:hover {
  background: var(--bg-hover);
}
.rule-row.disabled .row-value {
  color: var(--text-muted);
  text-decoration: line-through;
}
.row-index {
  width: 18px;
  flex-shrink: 0;
  text-align: right;
  font-size: 11px;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}
.row-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
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
.row-remark {
  font-size: 11px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.row-kind {
  flex-shrink: 0;
  font-size: 10px;
  color: var(--text-muted);
  border: 1px solid var(--border-color);
  border-radius: 3px;
  padding: 0 4px;
  white-space: nowrap;
}
.row-policy {
  flex-shrink: 0;
  font-size: 10px;
  border-radius: 3px;
  padding: 1px 5px;
}
.row-policy.vpn {
  color: #2563eb;
  background: rgba(59, 130, 246, 0.1);
}
.row-policy.direct {
  color: var(--text-secondary);
  background: var(--bg-secondary);
}
.row-policy-select {
  width: 84px;
  flex-shrink: 0;
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
  width: 20px;
  height: 20px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: transparent;
  color: var(--text-secondary);
  font-size: 11px;
  line-height: 1;
  cursor: pointer;
  padding: 0;
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
.rule-row-match {
  background: var(--bg-secondary);
  border-color: var(--border-color);
}
.match-spacer {
  width: 22px;
  flex-shrink: 0;
}
.rule-editor-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-top: 16px;
  padding-top: 12px;
  border-top: 1px solid var(--border-color);
}
.footer-hint {
  font-size: 12px;
  color: var(--text-muted);
}
</style>
