<script setup lang="ts">
import { computed, ref } from 'vue'
import type { VpnMode, VpnRule, VpnRuleSet } from '@/types/vpn'
import { MODE_DESCRIPTIONS } from '@/types/vpn'

const props = defineProps<{
  ruleSet: VpnRuleSet | null
  mode: VpnMode
  overlaps: string[]
}>()

const emit = defineEmits<{
  add: [cidr: string, remark: string]
  update: [ruleId: string, patch: Partial<VpnRule>]
  remove: [ruleId: string]
}>()

const newCidr = ref('')
const newRemark = ref('')
const error = ref('')

/** 输入时的即时校验，最终以后端规范化结果为准 */
const CIDR_PATTERN =
  /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\/(\d{1,2})$/

function validate(cidr: string): string {
  const text = cidr.trim()
  if (!text) return '请填写网段'
  const matched = CIDR_PATTERN.exec(text)
  if (!matched) return '格式应为 10.0.0.0/8 这样'
  const [, a, b, c, d, len] = matched
  const octets = [a, b, c, d].map((part) => Number(part))
  if (octets.some((value) => value > 255)) return '每一段必须在 0-255 之间'
  if (Number(len) > 32) return '前缀长度不能大于 32'
  return ''
}

function submit() {
  error.value = validate(newCidr.value)
  if (error.value) return
  emit('add', newCidr.value.trim(), newRemark.value.trim())
  newCidr.value = ''
  newRemark.value = ''
}

function onCellInput(ruleId: string, field: 'cidr' | 'remark', value: string) {
  emit('update', ruleId, { [field]: value } as Partial<VpnRule>)
}

const enabledCount = computed(
  () => props.ruleSet?.rules.filter((rule) => rule.enabled).length ?? 0,
)
const totalCount = computed(() => props.ruleSet?.rules.length ?? 0)
</script>

<template>
  <div class="rule-card">
    <div class="rule-head">
      <div>
        <h4>分流规则表</h4>
        <p class="rule-hint">
          <template v-if="mode === 'customer_first'">
            命中的网段走 VPN，未命中的走本地。{{ MODE_DESCRIPTIONS.customer_first }}
          </template>
          <template v-else>
            列出的网段直连本地，其余全部走 VPN。{{ MODE_DESCRIPTIONS.company_first }}
          </template>
        </p>
      </div>
      <span class="rule-count">{{ enabledCount }} / {{ totalCount }} 条启用</span>
    </div>

    <div v-if="!ruleSet" class="rule-empty">
      还没有绑定规则集，请先在上面选择一个配置
    </div>

    <template v-else>
      <div v-if="overlaps.length > 0" class="rule-warn">
        <div class="warn-title">规则覆盖提醒</div>
        <div v-for="(warning, index) in overlaps" :key="index" class="warn-item">
          {{ warning }}
        </div>
      </div>

      <div class="rule-table">
        <div class="rule-row rule-row-head">
          <span class="col-enabled">启用</span>
          <span class="col-cidr">网段 (CIDR)</span>
          <span class="col-remark">备注</span>
          <span class="col-actions"></span>
        </div>

        <div
          v-for="rule in ruleSet.rules"
          :key="rule.id"
          class="rule-row"
          :class="{ 'rule-row-off': !rule.enabled }"
        >
          <span class="col-enabled">
            <input
              type="checkbox"
              :checked="rule.enabled"
              @change="emit('update', rule.id, { enabled: ($event.target as HTMLInputElement).checked })"
            />
          </span>
          <span class="col-cidr">
            <input
              class="cell-input mono"
              :value="rule.cidr"
              spellcheck="false"
              @change="onCellInput(rule.id, 'cidr', ($event.target as HTMLInputElement).value)"
            />
          </span>
          <span class="col-remark">
            <input
              class="cell-input"
              :value="rule.remark"
              placeholder="例如 公司内网"
              @change="onCellInput(rule.id, 'remark', ($event.target as HTMLInputElement).value)"
            />
          </span>
          <span class="col-actions">
            <button class="row-delete" title="删除" @click="emit('remove', rule.id)">
              删除
            </button>
          </span>
        </div>

        <div v-if="ruleSet.rules.length === 0" class="rule-empty-row">
          还没有规则，在下面添加一条
        </div>
      </div>

      <div class="rule-add">
        <input
          v-model="newCidr"
          class="cell-input mono"
          placeholder="10.0.0.0/8"
          spellcheck="false"
          @keydown.enter="submit"
          @input="error = ''"
        />
        <input
          v-model="newRemark"
          class="cell-input"
          placeholder="备注（可选）"
          @keydown.enter="submit"
        />
        <button class="btn btn-primary" @click="submit">添加规则</button>
      </div>
      <p v-if="error" class="rule-error">{{ error }}</p>

      <div class="rule-presets">
        <span class="preset-label">常用网段：</span>
        <button
          v-for="preset in ['198.51.100.0/24', '192.168.1.0/24', '10.0.0.0/8', '172.16.0.0/12']"
          :key="preset"
          class="preset"
          @click="newCidr = preset"
        >
          {{ preset }}
        </button>
      </div>
    </template>
  </div>
</template>

<style scoped>
.rule-card {
  border: 1px solid var(--border-color);
  border-radius: 8px;
  padding: 14px;
  background: var(--bg-primary);
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
.rule-warn {
  border: 1px solid rgba(245, 158, 11, 0.35);
  background: rgba(245, 158, 11, 0.07);
  border-radius: 6px;
  padding: 8px 10px;
  margin-bottom: 10px;
}
.warn-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--warning);
  margin-bottom: 4px;
}
.warn-item {
  font-size: 12px;
  color: var(--text-secondary);
  line-height: 1.6;
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
.rule-row-off {
  opacity: 0.5;
}
.col-enabled {
  width: 40px;
  flex-shrink: 0;
  text-align: center;
}
.col-cidr {
  width: 200px;
  flex-shrink: 0;
}
.col-remark {
  flex: 1;
  min-width: 0;
}
.col-actions {
  width: 52px;
  flex-shrink: 0;
  text-align: right;
}
.cell-input {
  width: 100%;
  border: 1px solid transparent;
  background: transparent;
  border-radius: 4px;
  padding: 4px 6px;
  font-size: 13px;
  color: var(--text-primary);
  outline: none;
}
.cell-input:hover {
  border-color: var(--border-color);
}
.cell-input:focus {
  border-color: var(--primary);
  background: var(--bg-input);
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
.rule-empty-row,
.rule-empty {
  padding: 14px;
  text-align: center;
  font-size: 13px;
  color: var(--text-muted);
}
.rule-add {
  display: flex;
  gap: 8px;
  margin-top: 10px;
}
.rule-add .cell-input {
  border-color: var(--border-color);
  background: var(--bg-input);
  flex: 1;
}
.rule-add .cell-input:first-child {
  max-width: 200px;
}
.rule-error {
  margin-top: 6px;
  font-size: 12px;
  color: var(--danger);
}
.rule-presets {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 10px;
}
.preset-label {
  font-size: 12px;
  color: var(--text-muted);
}
.preset {
  border: 1px solid var(--border-color);
  background: var(--bg-input);
  border-radius: 10px;
  padding: 2px 9px;
  font-size: 12px;
  color: var(--text-secondary);
  cursor: pointer;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.preset:hover {
  border-color: var(--primary);
  color: var(--primary);
}
</style>
