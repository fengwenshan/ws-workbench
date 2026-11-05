<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { Button, Input, Select, SelectOption, Switch } from 'ant-design-vue'
import type { VpnPolicy, VpnRule, VpnRuleKind, VpnRuleSet } from '@/types/vpn'
import { POLICY_LABELS, RULE_KIND_HINTS, RULE_KIND_LABELS } from '@/types/vpn'

const props = defineProps<{
  ruleSet: VpnRuleSet
  /** 来自 useVpn 的逐条规则校验，失败时 reject */
  validateRule: (kind: VpnRuleKind, value: string) => Promise<string>
  /** 放进弹窗时隐藏卡片标题，只保留规则编辑区 */
  embedded?: boolean
}>()

const emit = defineEmits<{
  add: [kind: VpnRuleKind, value: string, policy: VpnPolicy, remark: string]
  update: [ruleId: string, patch: Partial<VpnRule>]
  remove: [ruleId: string]
  'update-default-policy': [policy: VpnPolicy]
}>()

const KINDS = Object.keys(RULE_KIND_LABELS) as VpnRuleKind[]
const POLICIES = Object.keys(POLICY_LABELS) as VpnPolicy[]

/** 下拉里同时显示 Clash 类型和中文含义，例如 DOMAIN（域名） */
function kindLabel(kind: VpnRuleKind) {
  return `${RULE_KIND_LABELS[kind]}（${RULE_KIND_HINTS[kind]}）`
}

/** 行内校验错误，按规则 id 记录 */
const errors = reactive<Record<string, string>>({})

async function ruleMessage(kind: VpnRuleKind, value: string): Promise<string> {
  if (!value.trim()) return '匹配值不能为空'
  try {
    await props.validateRule(kind, value)
    return ''
  } catch (err: any) {
    return String(err)
  }
}

function applyError(ruleId: string, message: string) {
  if (message) errors[ruleId] = message
  else delete errors[ruleId]
}

async function validateRow(rule: VpnRule) {
  applyError(rule.id, await ruleMessage(rule.kind, rule.value))
}

watch(
  () =>
    props.ruleSet.rules
      .map((rule) => [rule.id, rule.kind, rule.value].join('\u0000'))
      .join('\u0001'),
  () => {
    props.ruleSet.rules.forEach((rule) => validateRow(rule))
  },
  { immediate: true },
)

function onKindChange(rule: VpnRule, kind: VpnRuleKind) {
  const patch: Partial<VpnRule> = { kind }
  // no-resolve 只对 IP-CIDR 有意义，切走时复位
  if (kind !== 'IP-CIDR') patch.no_resolve = false
  emit('update', rule.id, patch)
  ruleMessage(kind, rule.value).then((message) => applyError(rule.id, message))
}

function onValueChange(rule: VpnRule, event: Event) {
  emit('update', rule.id, { value: (event.target as HTMLInputElement).value })
}

function onRemarkChange(rule: VpnRule, event: Event) {
  emit('update', rule.id, { remark: (event.target as HTMLInputElement).value })
}

const newKind = ref<VpnRuleKind>('DOMAIN')
const newValue = ref('')
const newPolicy = ref<VpnPolicy>('DIRECT')
const newRemark = ref('')
const addError = ref('')

async function submit() {
  const message = await ruleMessage(newKind.value, newValue.value)
  if (message) {
    addError.value = message
    return
  }
  emit('add', newKind.value, newValue.value.trim(), newPolicy.value, newRemark.value.trim())
  newValue.value = ''
  newRemark.value = ''
  addError.value = ''
}

/** 一键补上「域名含 corp 走 VPN」的默认规则，已有同义规则时不重复添加。 */
function presetCorp() {
  const exists = props.ruleSet.rules.some(
    (rule) =>
      rule.kind === 'DOMAIN-KEYWORD' &&
      rule.value.trim().toLowerCase() === 'corp' &&
      rule.policy === 'VPN',
  )
  if (!exists) {
    emit('add', 'DOMAIN-KEYWORD', 'corp', 'VPN', 'corp 相关域名走 VPN')
  }
}

const enabledCount = computed(
  () => props.ruleSet.rules.filter((rule) => rule.enabled).length,
)
const totalCount = computed(() => props.ruleSet.rules.length)
</script>

<template>
  <div class="rule-card" :class="{ 'rule-card-embedded': embedded }">
    <div v-if="!embedded" class="rule-head">
      <div>
        <h4>分流规则表</h4>
        <p class="rule-hint">
          规则自上而下匹配，命中即按该条的走向执行；全都不命中时走兜底的 MATCH 走向。
        </p>
      </div>
      <span class="rule-count">{{ enabledCount }} / {{ totalCount }} 条启用</span>
    </div>

    <div class="rule-table">
      <div class="rule-row rule-row-match">
        <span class="col-kind mono">MATCH</span>
        <span class="col-value match-hint">其余全部流量</span>
        <span class="col-policy">
          <Select
            size="small"
            class="cell-select"
            :value="ruleSet.default_policy"
            @change="(value: any) => emit('update-default-policy', value as VpnPolicy)"
          >
            <SelectOption v-for="policy in POLICIES" :key="policy" :value="policy">
              {{ POLICY_LABELS[policy] }}
            </SelectOption>
          </Select>
        </span>
        <span class="col-noresolve"></span>
        <span class="col-enabled"></span>
        <span class="col-remark"></span>
        <span class="col-actions"></span>
      </div>

      <div class="rule-row rule-row-head">
        <span class="col-kind">类型</span>
        <span class="col-value">匹配值</span>
        <span class="col-policy">走向</span>
        <span class="col-noresolve">no-resolve</span>
        <span class="col-enabled">启用</span>
        <span class="col-remark">备注</span>
        <span class="col-actions"></span>
      </div>

      <div
        v-for="rule in ruleSet.rules"
        :key="rule.id"
        class="rule-row"
        :class="{ 'rule-row-off': !rule.enabled, 'rule-row-error': !!errors[rule.id] }"
      >
        <span class="col-kind">
          <Select
            size="small"
            class="cell-select"
            :popup-match-select-width="true"
            :value="rule.kind"
            @change="(value: any) => onKindChange(rule, value as VpnRuleKind)"
          >
            <SelectOption v-for="kind in KINDS" :key="kind" :value="kind">
              {{ kindLabel(kind) }}
            </SelectOption>
          </Select>
        </span>
        <span class="col-value">
          <Input
            size="small"
            class="mono"
            :value="rule.value"
            spellcheck="false"
            @change="(event: Event) => onValueChange(rule, event)"
          />
          <span v-if="errors[rule.id]" class="cell-error">{{ errors[rule.id] }}</span>
        </span>
        <span class="col-policy">
          <Select
            size="small"
            class="cell-select"
            :value="rule.policy"
            @change="(value: any) => emit('update', rule.id, { policy: value as VpnPolicy })"
          >
            <SelectOption v-for="policy in POLICIES" :key="policy" :value="policy">
              {{ POLICY_LABELS[policy] }}
            </SelectOption>
          </Select>
        </span>
        <span class="col-noresolve">
          <Switch
            size="small"
            :checked="rule.no_resolve"
            :disabled="rule.kind !== 'IP-CIDR'"
            @change="(checked: boolean) => emit('update', rule.id, { no_resolve: checked })"
          />
        </span>
        <span class="col-enabled">
          <Switch
            size="small"
            :checked="rule.enabled"
            @change="(checked: boolean) => emit('update', rule.id, { enabled: checked })"
          />
        </span>
        <span class="col-remark">
          <Input
            size="small"
            :value="rule.remark"
            placeholder="备注（可选）"
            @change="(event: Event) => onRemarkChange(rule, event)"
          />
        </span>
        <span class="col-actions">
          <button class="row-delete" title="删除" @click="emit('remove', rule.id)">删除</button>
        </span>
      </div>

      <div v-if="ruleSet.rules.length === 0" class="rule-empty-row">
        还没有规则，在下面添加一条
      </div>
    </div>

    <div class="rule-add">
      <Select
        size="small"
        class="add-kind"
        :popup-match-select-width="true"
        v-model:value="newKind"
      >
        <SelectOption v-for="kind in KINDS" :key="kind" :value="kind">
          {{ kindLabel(kind) }}
        </SelectOption>
      </Select>
      <Input
        size="small"
        class="mono add-value"
        v-model:value="newValue"
        placeholder="匹配值，例如 10.0.0.0/8"
        spellcheck="false"
        @keydown.enter="submit"
        @change="addError = ''"
      />
      <Select size="small" class="add-policy" v-model:value="newPolicy">
        <SelectOption v-for="policy in POLICIES" :key="policy" :value="policy">
          {{ POLICY_LABELS[policy] }}
        </SelectOption>
      </Select>
      <Input
        size="small"
        class="add-remark"
        v-model:value="newRemark"
        placeholder="备注（可选）"
        @keydown.enter="submit"
      />
      <Button type="primary" size="small" @click="submit">新增规则</Button>
      <Button size="small" @click="presetCorp">corp → VPN</Button>
    </div>
    <p v-if="addError" class="rule-error">{{ addError }}</p>
  </div>
</template>

<style scoped>
.rule-card {
  border: 1px solid var(--border-color);
  border-radius: 8px;
  padding: 14px;
  background: var(--bg-primary);
}
.rule-card-embedded {
  border: none;
  border-radius: 0;
  padding: 0;
}
.rule-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 12px;
}
.rule-head h4 {
  font-size: 14px;
  color: var(--text-primary);
  margin-bottom: 4px;
}
.rule-hint {
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
  max-width: 620px;
}
.rule-count {
  font-size: 12px;
  color: var(--text-muted);
  white-space: nowrap;
  flex-shrink: 0;
}
.rule-table {
  border: 1px solid var(--border-color);
  border-radius: 6px;
  overflow: hidden;
}
.rule-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 8px;
  border-bottom: 1px solid var(--border-color);
}
.rule-row:last-child {
  border-bottom: none;
}
.rule-row-head {
  background: var(--bg-secondary);
  font-size: 12px;
  color: var(--text-muted);
}
.rule-row-match {
  background: var(--bg-active);
}
.rule-row-match .match-hint {
  font-size: 12px;
  color: var(--text-muted);
}
.rule-row-off {
  opacity: 0.5;
}
.rule-row-error {
  background: rgba(239, 68, 68, 0.06);
}
.col-kind {
  width: 260px;
  flex-shrink: 0;
}
.col-value {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.col-policy {
  width: 104px;
  flex-shrink: 0;
}
.col-noresolve {
  width: 72px;
  flex-shrink: 0;
  text-align: center;
}
.col-enabled {
  width: 52px;
  flex-shrink: 0;
  text-align: center;
}
.col-remark {
  width: 168px;
  flex-shrink: 0;
}
.col-actions {
  width: 44px;
  flex-shrink: 0;
  text-align: right;
}
.cell-select {
  width: 100%;
}
.cell-error {
  font-size: 11px;
  color: var(--danger);
  line-height: 1.4;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.row-delete {
  border: none;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  cursor: pointer;
  padding: 2px 4px;
}
.row-delete:hover {
  color: var(--danger);
}
.rule-empty-row {
  padding: 14px;
  text-align: center;
  font-size: 13px;
  color: var(--text-muted);
}
.rule-add {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 10px;
}
.add-kind {
  width: 260px;
  flex-shrink: 0;
}
.add-value {
  flex: 1;
  min-width: 0;
}
.add-policy {
  width: 104px;
  flex-shrink: 0;
}
.add-remark {
  width: 168px;
  flex-shrink: 0;
}
.rule-error {
  margin-top: 6px;
  font-size: 12px;
  color: var(--danger);
}
</style>
