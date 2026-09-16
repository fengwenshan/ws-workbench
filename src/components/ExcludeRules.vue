<script setup lang="ts">
import { ref } from 'vue'

const props = defineProps<{
  modelValue: string[]
  placeholder?: string
}>()

const emit = defineEmits<{
  'update:modelValue': [value: string[]]
}>()

const input = ref('')

function addRule() {
  const value = input.value.trim()
  if (value && !props.modelValue.includes(value)) {
    emit('update:modelValue', [...props.modelValue, value])
  }
  input.value = ''
}

function removeRule(index: number) {
  const rules = [...props.modelValue]
  rules.splice(index, 1)
  emit('update:modelValue', rules)
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' || e.key === ',') {
    e.preventDefault()
    addRule()
  }
}
</script>

<template>
  <div class="exclude-rules">
    <div class="tags">
      <span v-for="(rule, index) in modelValue" :key="index" class="tag">
        {{ rule }}
        <button class="tag-remove" @click="removeRule(index)">×</button>
      </span>
    </div>
    <input
      v-model="input"
      class="rule-input"
      :placeholder="placeholder || '输入排除规则后回车添加'"
      @keydown="onKeydown"
      @blur="addRule"
    />
  </div>
</template>

<style scoped>
.exclude-rules {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  padding: 6px 8px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  min-height: 38px;
  background: var(--bg-input);
}
.tags {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.tag {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  background: var(--bg-tag);
  border-radius: 4px;
  font-size: 13px;
  color: var(--text-tag);
}
.tag-remove {
  border: none;
  background: none;
  cursor: pointer;
  color: var(--text-muted);
  font-size: 16px;
  line-height: 1;
  padding: 0;
}
.tag-remove:hover {
  color: var(--danger);
}
.rule-input {
  flex: 1;
  min-width: 150px;
  border: none;
  outline: none;
  background: transparent;
  font-size: 13px;
  color: var(--text-primary);
}
</style>
