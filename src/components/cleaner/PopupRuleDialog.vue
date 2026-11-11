<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Button, Checkbox, Modal, Select } from 'ant-design-vue'
import AppIcon from './AppIcon.vue'
import type {
  PopupAction,
  PopupHit,
  PopupRuleDraft,
  PopupScope,
  PopupWindow,
} from '@/types/popup'

/**
 * 「拦截这个弹窗」确认框。
 *
 * 照火绒的口径把选择权交给用户：只拦这一个窗口 / 拦该程序的同类弹窗 /
 * 拦该程序的所有弹窗。默认选「同类弹窗」——同一个广告位换标题、换素材也能拦住，
 * 又不会把程序的主窗口一起关掉。
 */
const props = defineProps<{
  open: boolean
  /** 被拦截的窗口（来自可疑记录或窗口选择器） */
  target: PopupHit | PopupWindow | null
  /** 设置里保存的默认动作 */
  action: PopupAction
  /** 拦截线程是否在跑：没跑就不能「立即处理」 */
  running: boolean
  busy: boolean
  appName: (path: string, process: string) => string
  appIcon: (path: string) => string | null
}>()

const emit = defineEmits<{
  'update:open': [boolean]
  submit: [draft: PopupRuleDraft, applyNow: boolean]
}>()

const scope = ref<PopupScope>('class')
const action = ref<PopupAction>('close')
const applyNow = ref(true)

const title = computed(() => props.target?.title ?? '')
const className = computed(() => props.target?.class_name ?? '')

const scopeOptions = computed<
  Array<{ value: PopupScope; title: string; desc: string; disabled: boolean }>
>(() => [
  {
    value: 'class',
    title: '该程序的同类弹窗（推荐）',
    desc: className.value
      ? `按「进程 + 窗口类」拦截：${className.value}`
      : '这个窗口没有窗口类名，无法按同类拦截',
    disabled: !className.value,
  },
  {
    value: 'window',
    title: '仅这一个弹窗',
    desc: title.value
      ? `进程 + 窗口类 + 当前标题「${title.value}」`
      : '这个窗口没有标题，无法只按标题拦截',
    disabled: !title.value,
  },
  {
    value: 'app',
    title: '该程序的所有弹窗',
    desc: `只处理 ${props.target?.process ?? ''} 弹出的广告 / 附属窗口，应用主窗口会自动跳过`,
    disabled: false,
  },
])

const actionOptions = [
  { value: 'close', label: '关闭窗口（推荐）' },
  { value: 'hide', label: '隐藏窗口' },
]

watch(
  () => props.open,
  (open) => {
    if (!open) return
    scope.value = props.target?.class_name ? 'class' : props.target?.title ? 'window' : 'app'
    action.value = props.action
    applyNow.value = props.running
  },
)

const draft = computed<PopupRuleDraft>(() => ({
  process: props.target?.process ?? '',
  path: props.target?.path ?? '',
  class_name: scope.value === 'app' ? '' : (props.target?.class_name ?? ''),
  title: scope.value === 'window' ? (props.target?.title ?? '') : '',
  position: props.target?.position ?? '',
  scope: scope.value,
  action: action.value,
}))

function close(): void {
  if (props.busy) return
  emit('update:open', false)
}

function onActionChange(value: unknown): void {
  action.value = value === 'hide' ? 'hide' : 'close'
}

function onApplyNowChange(event: { target?: { checked?: boolean } }): void {
  applyNow.value = !!event?.target?.checked
}

function submit(): void {
  if (props.busy || !props.target) return
  emit('submit', draft.value, applyNow.value && props.running)
}
</script>

<template>
  <Modal
    :open="open"
    title="添加拦截规则"
    :width="480"
    :mask-closable="!busy"
    :closable="!busy"
    @cancel="close"
  >
    <div v-if="target" class="rule-dialog">
      <div class="target">
        <AppIcon
          :name="appName(target.path, target.process)"
          :icon="appIcon(target.path)"
          :size="38"
        />
        <div class="target-main">
          <div class="target-name">
            {{ appName(target.path, target.process) }}
            <span class="target-process">{{ target.process }}</span>
          </div>
          <div class="target-title" :title="target.title">
            {{ target.title || '（无标题窗口）' }}
          </div>
        </div>
      </div>

      <div class="field">
        <div class="field-label">拦截范围</div>
        <button
          v-for="option in scopeOptions"
          :key="option.value"
          type="button"
          class="scope"
          :class="{ on: scope === option.value, disabled: option.disabled }"
          :disabled="option.disabled"
          @click="scope = option.value"
        >
          <span class="scope-radio" :class="{ on: scope === option.value }" />
          <span class="scope-text">
            <b>{{ option.title }}</b>
            <em>{{ option.desc }}</em>
          </span>
        </button>
        <p v-if="scope === 'app'" class="scope-warn">
          应用主窗口会自动跳过；但该程序的其他正常窗口（设置、文档）也会被一起关闭，慎用。
        </p>
      </div>

      <div class="options">
        <div class="option-row">
          <span class="field-label">处理方式</span>
          <Select
            :value="action"
            :options="actionOptions"
            size="small"
            style="width: 168px"
            @change="onActionChange"
          />
        </div>
        <label class="apply-now" :class="{ disabled: !running }">
          <Checkbox
            :checked="applyNow"
            :disabled="!running"
            @change="onApplyNowChange"
          />
          <span>立即拦截当前已经打开的同类窗口</span>
          <span v-if="!running" class="apply-hint">（先开启弹窗拦截）</span>
        </label>
      </div>
    </div>

    <template #footer>
      <div class="dialog-footer">
        <Button :disabled="busy" @click="close">取消</Button>
        <Button type="primary" :loading="busy" @click="submit">保存规则</Button>
      </div>
    </template>
  </Modal>
</template>

<style scoped>
.rule-dialog {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.target {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px;
  border: 1px solid var(--border-color);
  border-radius: 10px;
  background: var(--bg-secondary);
}

.target-main {
  min-width: 0;
}

.target-name {
  display: flex;
  align-items: baseline;
  gap: 8px;
  color: var(--text-primary);
  font-size: 14px;
  font-weight: 600;
}

.target-process {
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 400;
}

.target-title {
  margin-top: 3px;
  color: var(--text-secondary);
  font-size: 12px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.field-label {
  color: var(--text-secondary);
  font-size: 12px;
}

.scope {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 10px 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  text-align: left;
  cursor: pointer;
  transition: border-color 0.15s, background 0.15s;
}

.scope:hover:not(.disabled) {
  border-color: var(--primary);
}

.scope.on {
  border-color: var(--primary);
  background: var(--bg-active);
}

.scope.disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.scope-radio {
  flex-shrink: 0;
  width: 14px;
  height: 14px;
  margin-top: 2px;
  border: 1px solid var(--text-muted);
  border-radius: 50%;
  background: var(--bg-primary);
}

.scope-radio.on {
  border: 4px solid var(--primary);
}

.scope-text {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}

.scope-text b {
  color: var(--text-primary);
  font-size: 13px;
  font-weight: 500;
}

.scope-text em {
  color: var(--text-muted);
  font-size: 11px;
  font-style: normal;
  line-height: 1.5;
  word-break: break-all;
}

.scope-warn {
  margin: 0;
  padding: 6px 10px;
  border-radius: 6px;
  background: rgba(245, 158, 11, 0.1);
  color: #b45309;
  font-size: 11px;
  line-height: 1.5;
}

.options {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.option-row {
  display: flex;
  align-items: center;
  gap: 12px;
}

.apply-now {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
}

.apply-now.disabled {
  color: var(--text-muted);
  cursor: not-allowed;
}

.apply-hint {
  color: var(--text-muted);
}

.dialog-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
</style>
