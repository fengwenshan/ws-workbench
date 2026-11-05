<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Button, Input, Modal } from 'ant-design-vue'
import type { VpnPolicy, VpnRule, VpnRuleKind, VpnRuleSet } from '@/types/vpn'
import VpnRuleTable from './VpnRuleTable.vue'

const props = defineProps<{
  ruleSet: VpnRuleSet
  validateRule: (kind: VpnRuleKind, value: string) => Promise<string>
}>()

const emit = defineEmits<{
  close: []
  rename: [name: string]
  add: [kind: VpnRuleKind, value: string, policy: VpnPolicy, remark: string]
  update: [ruleId: string, patch: Partial<VpnRule>]
  remove: [ruleId: string]
  'update-default-policy': [policy: VpnPolicy]
}>()

const nameInput = ref(props.ruleSet.name)

watch(
  () => props.ruleSet.id,
  () => {
    nameInput.value = props.ruleSet.name
  },
)
watch(
  () => props.ruleSet.name,
  (name) => {
    nameInput.value = name
  },
)

const enabledCount = computed(
  () => props.ruleSet.rules.filter((rule) => rule.enabled).length,
)

function commitName() {
  const name = nameInput.value.trim()
  if (!name) {
    nameInput.value = props.ruleSet.name
    return
  }
  if (name !== props.ruleSet.name) emit('rename', name)
}

function finish() {
  commitName()
  emit('close')
}

function onAdd(kind: VpnRuleKind, value: string, policy: VpnPolicy, remark: string) {
  emit('add', kind, value, policy, remark)
}

function onUpdate(ruleId: string, patch: Partial<VpnRule>) {
  emit('update', ruleId, patch)
}

function onRemove(ruleId: string) {
  emit('remove', ruleId)
}

function onDefaultPolicy(policy: VpnPolicy) {
  emit('update-default-policy', policy)
}
</script>

<template>
  <Modal
    :open="true"
    title="编辑规则列表"
    :width="1040"
    :footer="null"
    :mask-closable="false"
    :body-style="{ padding: '0', maxHeight: '76vh', overflow: 'auto' }"
    wrap-class-name="vpn-rule-dialog"
    @cancel="finish"
  >
    <p class="rule-dialog-desc">
      规则从上到下匹配，命中第一条后即停止；没有命中时走 MATCH 的走向。
    </p>

    <div class="rule-dialog-name">
      <label for="rule-set-name">列表名称</label>
      <Input
        id="rule-set-name"
        v-model:value="nameInput"
        class="rule-dialog-name-input"
        :maxlength="60"
        placeholder="例如 公司内网、常用分流"
        @blur="commitName"
        @keydown.enter="commitName"
      />
      <span class="rule-dialog-count">{{ enabledCount }} / {{ ruleSet.rules.length }} 条启用</span>
    </div>

    <div class="rule-dialog-body">
      <VpnRuleTable
        :rule-set="ruleSet"
        :validate-rule="validateRule"
        embedded
        @add="onAdd"
        @update="onUpdate"
        @remove="onRemove"
        @update-default-policy="onDefaultPolicy"
      />
    </div>

    <div class="rule-dialog-foot">
      <span>这份列表只有被设为当前生效列表时才会参与分流。</span>
      <Button type="primary" @click="finish">完成</Button>
    </div>
  </Modal>
</template>

<style scoped>
.rule-dialog-desc {
  margin: 0;
  padding: 0 24px 10px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}
.rule-dialog-name {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 24px;
  border-top: 1px solid var(--border-color);
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}
.rule-dialog-name label {
  font-size: 12px;
  color: var(--text-secondary);
  flex-shrink: 0;
}
.rule-dialog-name-input {
  width: 280px;
  max-width: 50%;
}
.rule-dialog-count {
  margin-left: auto;
  font-size: 12px;
  color: var(--text-muted);
  white-space: nowrap;
}
.rule-dialog-body {
  padding: 14px 24px 18px;
}
.rule-dialog-body :deep(.rule-table) {
  min-width: 760px;
}
.rule-dialog-foot {
  position: sticky;
  bottom: 0;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 12px 24px;
  border-top: 1px solid var(--border-color);
  background: var(--bg-primary);
}
.rule-dialog-foot span {
  font-size: 12px;
  color: var(--text-muted);
}
</style>
