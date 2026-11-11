<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Checkbox, Select, Switch, message } from 'ant-design-vue'
import AppIcon from './AppIcon.vue'
import PopupRuleDialog from './PopupRuleDialog.vue'
import PopupShield from './PopupShield.vue'
import PopupWindowPicker from './PopupWindowPicker.vue'
import { usePopup } from '@/composables/usePopup'
import type {
  PopupAction,
  PopupHit,
  PopupRule,
  PopupRuleDraft,
  PopupWindow,
} from '@/types/popup'

/**
 * 弹窗拦截页签（仅 Windows）。
 *
 * 对齐火绒的形态：按「软件」分组列出弹窗规则，每个软件一行总开关，展开后是
 * 具体规则（名称 / 位置 / 拦截次数 / 开关）。规则只处理应用弹出的广告 / 附属
 * 窗口，应用主窗口由后端强制跳过；没有规则的窗口进「窗口记录」等用户确认。
 */
const props = defineProps<{
  active: boolean
}>()

const popup = usePopup()

type ViewKey = 'rules' | 'pending' | 'blocked'
const view = ref<ViewKey>('rules')
/** 火绒的「隐藏未触发规则」 */
const hideUnused = ref(false)
/** 展开的软件分组（进程名小写） */
const expanded = ref<Set<string>>(new Set())

interface RuleGroup {
  process: string
  path: string
  name: string
  icon: string | null
  rules: PopupRule[]
  /** 分组下规则总数 / 启用数（不受「隐藏未触发」影响） */
  total: number
  enabledTotal: number
  hits: number
  allEnabled: boolean
}

const ACTION_OPTIONS = [
  { value: 'close', label: '关闭窗口' },
  { value: 'hide', label: '隐藏窗口' },
]
const SCORE_OPTIONS = [
  { value: 6, label: '6' },
  { value: 8, label: '8' },
  { value: 10, label: '10' },
  { value: 12, label: '12' },
  { value: 15, label: '15' },
]

const tabs = computed(() => [
  { key: 'rules' as const, label: '拦截规则', count: popup.rules.length },
  { key: 'pending' as const, label: '窗口记录', count: popup.pendingHits.length },
  { key: 'blocked' as const, label: '拦截记录', count: popup.blockedHits.length },
])

const disabledRules = computed(() => popup.rules.filter((rule) => !rule.enabled).length)

const heroHint = computed(() => {
  if (!popup.running) {
    return '开启后自动关闭命中规则的应用弹窗；应用主窗口不会被碰。'
  }
  if (!popup.rules.length) {
    return '还没有规则：点「从窗口添加」把正在弹的广告窗加进来。'
  }
  if (disabledRules.value) {
    return `有 ${disabledRules.value} 条规则已停用，其余规则自动拦截应用弹出的广告窗口。`
  }
  return '规则已全部开启，自动拦截应用弹出的广告窗口；应用主窗口不会被碰。'
})

/** 按进程分组，顺序按累计拦截次数、规则数排序（火绒的软件列表） */
const groups = computed<RuleGroup[]>(() => {
  const map = new Map<string, RuleGroup>()
  for (const rule of popup.rules) {
    const key = rule.process.toLowerCase()
    let group = map.get(key)
    if (!group) {
      group = {
        process: rule.process,
        path: rule.path,
        name: rule.process.replace(/\.exe$/i, ''),
        icon: null,
        rules: [],
        total: 0,
        enabledTotal: 0,
        hits: 0,
        allEnabled: true,
      }
      map.set(key, group)
    }
    if (!group.path && rule.path) group.path = rule.path
    group.rules.push(rule)
    group.hits += rule.hits
  }

  const list = [...map.values()]
  for (const group of list) {
    group.rules.sort(
      (a, b) =>
        Number(b.enabled) - Number(a.enabled) ||
        b.hits - a.hits ||
        b.created_ms - a.created_ms,
    )
    group.total = group.rules.length
    group.enabledTotal = group.rules.filter((rule) => rule.enabled).length
    group.allEnabled = group.enabledTotal === group.total
    group.name = popup.displayName(group.path, group.process)
    group.icon = popup.iconOf(group.path)
  }

  const visible = hideUnused.value
    ? list
        .map((group) => ({ ...group, rules: group.rules.filter((rule) => rule.hits > 0) }))
        .filter((group) => group.rules.length > 0)
    : list
  return visible.sort(
    (a, b) =>
      b.hits - a.hits ||
      b.total - a.total ||
      a.name.localeCompare(b.name, 'zh-CN'),
  )
})

const ruleDialogOpen = ref(false)
const ruleTarget = ref<PopupHit | PopupWindow | null>(null)
const pickerOpen = ref(false)

watch(
  () => props.active,
  (active) => {
    if (active) {
      void popup.load()
      popup.startPolling()
    } else {
      popup.stopPolling()
    }
  },
  { immediate: true },
)

watch(pickerOpen, (open) => {
  if (open) void popup.loadWindows()
})

async function onToggle(checked: boolean): Promise<void> {
  const ok = await popup.toggle(checked)
  if (!ok) {
    if (popup.errorMessage) message.error(popup.errorMessage)
  } else {
    message.success(checked ? '弹窗拦截已开启，规则开始生效' : '弹窗拦截已关闭')
  }
}

async function onClear(): Promise<void> {
  const ok = await popup.clear()
  if (ok) message.success('已清空记录')
  else if (popup.errorMessage) message.error(popup.errorMessage)
}

async function refreshPending(): Promise<void> {
  await popup.refreshRecent()
  message.success('窗口记录已刷新')
}

function toggleExpand(process: string): void {
  const next = new Set(expanded.value)
  if (next.has(process)) next.delete(process)
  else next.add(process)
  expanded.value = next
}

async function dismissHit(hit: PopupHit, action: PopupAction): Promise<void> {
  const ok = await popup.dismiss(hit.id, action)
  if (ok) message.success(action === 'close' ? '已请求关闭这个弹窗' : '已隐藏这个弹窗')
  else message.error(popup.errorMessage || '处理失败，窗口可能已经关闭')
}

async function ignoreHit(hit: PopupHit): Promise<void> {
  const ok = await popup.ignore(hit.process, true)
  if (ok) {
    message.success(`已忽略 ${popup.displayName(hit.path, hit.process)}，以后不再检测它的弹窗`)
    await popup.refreshRecent()
  } else if (popup.errorMessage) {
    message.error(popup.errorMessage)
  }
}

function blockHit(hit: PopupHit): void {
  ruleTarget.value = hit
  ruleDialogOpen.value = true
}

function openPicker(): void {
  pickerOpen.value = true
}

function pickWindow(target: PopupWindow): void {
  pickerOpen.value = false
  ruleTarget.value = target
  ruleDialogOpen.value = true
}

async function submitRule(draft: PopupRuleDraft, applyNow: boolean): Promise<void> {
  const ok = await popup.addRule(draft, applyNow)
  if (!ok) {
    if (popup.errorMessage) message.error(popup.errorMessage)
    return
  }
  ruleDialogOpen.value = false
  view.value = applyNow ? 'blocked' : 'rules'
  if (!applyNow) {
    // 新加的规则默认展开它所属的软件，用户能立刻看到
    expanded.value = new Set([...expanded.value, draft.process.toLowerCase()])
  }
  message.success(
    applyNow ? '已添加拦截规则，当前打开的同类窗口也已处理' : '已添加拦截规则，下次出现时自动拦截',
  )
}

async function toggleRule(rule: PopupRule, enabled: boolean): Promise<void> {
  const ok = await popup.toggleRule(rule.id, enabled)
  if (!ok && popup.errorMessage) message.error(popup.errorMessage)
}

async function toggleGroup(group: RuleGroup, enabled: boolean): Promise<void> {
  const ok = await popup.toggleApp(group.process, enabled)
  if (!ok) {
    if (popup.errorMessage) message.error(popup.errorMessage)
    return
  }
  message.success(
    enabled
      ? `已开启「${group.name}」的 ${group.total} 条规则`
      : `已停用「${group.name}」的 ${group.total} 条规则`,
  )
}

async function removeRule(rule: PopupRule): Promise<void> {
  const ok = await popup.removeRule(rule.id)
  if (ok) message.success('已删除拦截规则')
  else if (popup.errorMessage) message.error(popup.errorMessage)
}

async function runRule(rule: PopupRule): Promise<void> {
  const applied = await popup.applyRule(rule.id)
  if (applied === null) message.error(popup.errorMessage || '执行失败')
  else if (applied > 0) message.success(`已处理 ${applied} 个窗口`)
  else message.info('当前没有匹配的窗口')
}

async function setIgnore(process: string, ignored: boolean): Promise<void> {
  const ok = await popup.ignore(process, ignored)
  if (ok) message.success(ignored ? `已忽略 ${process}` : `已取消忽略 ${process}`)
  else if (popup.errorMessage) message.error(popup.errorMessage)
}

async function updateAction(value: unknown): Promise<void> {
  const action: PopupAction = value === 'hide' ? 'hide' : 'close'
  const ok = await popup.saveSettings({ action })
  if (!ok && popup.errorMessage) message.error(popup.errorMessage)
}

async function toggleSmart(checked: boolean): Promise<void> {
  const ok = await popup.saveSettings({ smart: checked })
  if (!ok && popup.errorMessage) message.error(popup.errorMessage)
}

async function setMinScore(value: unknown): Promise<void> {
  const score = Number(value)
  if (!Number.isFinite(score)) return
  const ok = await popup.saveSettings({ min_score: score })
  if (!ok && popup.errorMessage) message.error(popup.errorMessage)
}

function formatTime(ms: number): string {
  if (!ms) return '—'
  const date = new Date(ms)
  const time = date.toLocaleTimeString('zh-CN', { hour12: false })
  const now = new Date()
  if (date.toDateString() === now.toDateString()) return time
  return `${date.getMonth() + 1}-${date.getDate()} ${time.slice(0, 5)}`
}

function actionLabel(action: string | null | undefined): string {
  return action === 'hide' ? '已隐藏' : '已关闭'
}

/** 规则里的动作是配置，不是「已经发生」，不加「已」 */
function ruleActionLabel(action: string | null | undefined): string {
  return action === 'hide' ? '隐藏窗口' : '关闭窗口'
}

function sourceLabel(hit: PopupHit): string {
  if (hit.source === 'smart') return '智能拦截'
  return '规则拦截'
}

function reasonText(hit: PopupHit): string {
  return hit.reasons.slice(0, 2).join('；')
}

function scopeLabel(rule: PopupRule): string {
  if (rule.scope === 'app') return '全部弹窗'
  if (rule.scope === 'window') return '仅此弹窗'
  return '同类弹窗'
}

function scopeDetail(rule: PopupRule): string {
  const parts: string[] = []
  if (rule.class_name) parts.push(`类名 ${rule.class_name}`)
  if (rule.title) parts.push(`标题含「${rule.title}」`)
  return parts.length ? parts.join(' · ') : '该程序的所有弹窗'
}
</script>

<template>
  <div class="popup">
    <section class="hero" :class="{ on: popup.running }">
      <PopupShield :size="42" :active="popup.running" />
      <div class="hero-text">
        <div class="hero-title">
          <h4>{{ popup.totalHits }} 项弹窗已拦截</h4>
          <span class="hero-state" :class="{ on: popup.running }">
            {{ popup.running ? '已开启' : '已关闭' }}
          </span>
        </div>
        <p class="hero-hint">{{ heroHint }}</p>
      </div>
      <div class="hero-switch">
        <span class="hero-switch-label" :class="{ on: popup.running }">
          {{ popup.running ? '拦截已生效' : '拦截未生效' }}
        </span>
        <Switch
          :checked="popup.running"
          :loading="popup.busy"
          :disabled="popup.busy || !popup.supported"
          @change="onToggle"
        />
      </div>
    </section>

    <div class="stats">
      <div class="stat">
        <b>{{ popup.totalHits }}</b>
        <span>累计拦截</span>
      </div>
      <div class="stat">
        <b>{{ popup.appCount }}</b>
        <span>覆盖应用</span>
      </div>
      <div class="stat">
        <b>{{ popup.rules.length }}</b>
        <span>拦截规则</span>
      </div>
      <div class="stat stat-wide">
        <span class="stat-muted">最近拦截</span>
        <b class="stat-time">{{ formatTime(popup.lastHitAt) }}</b>
      </div>
    </div>

    <div class="subbar">
      <div class="segmented">
        <button
          v-for="tab in tabs"
          :key="tab.key"
          type="button"
          class="segmented-item"
          :class="{ active: view === tab.key }"
          @click="view = tab.key"
        >
          {{ tab.label }}
          <span class="segmented-count">{{ tab.count }}</span>
        </button>
      </div>
      <div class="subbar-actions">
        <button
          v-if="view === 'rules'"
          type="button"
          class="btn btn-primary btn-sm"
          @click="openPicker"
        >
          从窗口添加
        </button>
        <button
          v-else-if="view === 'pending'"
          type="button"
          class="btn btn-secondary btn-sm"
          @click="refreshPending"
        >
          刷新记录
        </button>
        <button
          v-else-if="view === 'blocked'"
          type="button"
          class="btn btn-secondary btn-sm"
          :disabled="!popup.blockedHits.length"
          @click="onClear"
        >
          清空记录
        </button>
      </div>
    </div>

    <p v-if="popup.errorMessage" class="popup-error">{{ popup.errorMessage }}</p>

    <div class="body">
      <div v-if="!popup.supported && !popup.loading" class="empty">
        <PopupShield :size="52" />
        <div class="empty-title">仅 Windows 支持弹窗拦截</div>
      </div>

      <div v-else-if="popup.loading" class="empty">
        <div class="empty-title">正在读取拦截状态…</div>
      </div>

      <!-- 拦截记录 -->
      <template v-else-if="view === 'blocked'">
        <div v-if="!popup.blockedHits.length" class="empty">
          <PopupShield :size="56" :active="popup.running" />
          <div class="empty-title">
            {{ popup.running ? '还没有拦截记录' : '弹窗拦截还没开启' }}
          </div>
          <p class="empty-desc">
            {{
              popup.running
                ? '命中规则的弹窗会自动关闭并记录在这里；没有规则时先去「窗口记录」点一次「拦截此类」。'
                : '打开右上角开关后，命中规则的弹窗会在出现时自动关闭。'
            }}
          </p>
          <div class="empty-actions">
            <button type="button" class="btn btn-secondary" @click="view = 'rules'">
              添加拦截规则
            </button>
            <button type="button" class="btn btn-secondary" @click="openPicker">
              从正在运行的窗口添加
            </button>
          </div>
        </div>
        <div v-else class="list">
          <div v-for="hit in popup.blockedHits" :key="hit.id" class="row">
            <AppIcon
              :name="popup.displayName(hit.path, hit.process)"
              :icon="popup.iconOf(hit.path)"
              :size="30"
            />
            <div class="row-main">
              <div class="row-title" :title="hit.title">
                {{ hit.title || '（无标题窗口）' }}
              </div>
              <div class="row-sub">
                {{ popup.displayName(hit.path, hit.process) }} · {{ hit.process }} ·
                {{ reasonText(hit) }}
              </div>
            </div>
            <span class="tag" :class="hit.auto_action === 'hide' ? 'tag-hide' : 'tag-block'">
              {{ actionLabel(hit.auto_action) }}
            </span>
            <span class="tag tag-source">{{ sourceLabel(hit) }}</span>
            <span class="row-time">{{ formatTime(hit.at_ms) }}</span>
          </div>
        </div>
      </template>

      <!-- 拦截规则：按软件分组（火绒式列表） -->
      <template v-else-if="view === 'rules'">
        <div class="rules-toolbar">
          <span class="rules-summary">
            共 {{ popup.rules.length }} 条规则 · {{ popup.appCount }} 个应用
          </span>
          <Checkbox v-model:checked="hideUnused">隐藏未触发规则</Checkbox>
        </div>

        <div v-if="!popup.rules.length" class="empty">
          <PopupShield :size="56" />
          <div class="empty-title">还没有拦截规则</div>
          <p class="empty-desc">
            应用弹出的广告窗出现时，点一次「拦截此类」就会生成规则；之后同类弹窗一出现
            就会被自动关闭。也可以直接从当前正在运行的窗口里挑一个。
          </p>
          <div class="empty-actions">
            <button type="button" class="btn btn-secondary" @click="openPicker">
              从正在运行的窗口添加
            </button>
            <button type="button" class="btn btn-secondary" @click="view = 'pending'">
              去窗口记录里拦截
            </button>
          </div>
        </div>

        <div v-else-if="!groups.length" class="empty">
          <PopupShield :size="56" :active="popup.running" />
          <div class="empty-title">所有规则都还没触发过</div>
          <p class="empty-desc">取消勾选「隐藏未触发规则」就能看到它们。</p>
        </div>

        <div v-else class="rule-table">
          <div class="rule-head">
            <span>软件弹窗名称</span>
            <span>弹窗位置</span>
            <span>拦截次数</span>
            <span>操作</span>
          </div>

          <template v-for="group in groups" :key="group.process">
            <div class="app-row" @click="toggleExpand(group.process)">
              <div class="app-cell">
                <AppIcon :name="group.name" :icon="group.icon" :size="26" />
                <div class="app-main">
                  <div class="app-name">
                    {{ group.name }}
                    <span class="app-process">{{ group.process }}</span>
                  </div>
                  <div class="app-sub">
                    共 {{ group.total }} 项
                    <template v-if="group.enabledTotal < group.total">
                      · {{ group.enabledTotal }} 条启用
                    </template>
                  </div>
                </div>
                <svg
                  class="chevron"
                  :class="{ open: expanded.has(group.process) }"
                  viewBox="0 0 16 16"
                  aria-hidden="true"
                >
                  <path d="M4 6l4 4 4-4" />
                </svg>
              </div>
              <span class="cell-muted">—</span>
              <span class="cell-hits">{{ group.hits }} 次</span>
              <div class="cell-op" @click.stop>
                <Switch
                  size="small"
                  :checked="group.allEnabled"
                  @change="(checked) => toggleGroup(group, checked === true)"
                />
              </div>
            </div>

            <div v-if="expanded.has(group.process)" class="rule-lines">
              <div v-for="rule in group.rules" :key="rule.id" class="rule-line">
                <div class="rule-name-cell">
                  <div class="rule-name">{{ rule.name || '广告弹窗' }}</div>
                  <div class="rule-meta" :title="scopeDetail(rule)">
                    <span class="tag tag-scope">{{ scopeLabel(rule) }}</span>
                    <span class="rule-detail">{{ scopeDetail(rule) }}</span>
                    <span class="rule-action">{{ ruleActionLabel(rule.action) }}</span>
                  </div>
                </div>
                <span class="cell-muted">{{ rule.position || '任意位置' }}</span>
                <span class="cell-hits">{{ rule.hits }} 次</span>
                <div class="cell-op">
                  <button
                    type="button"
                    class="link-btn"
                    :disabled="!rule.enabled"
                    title="对当前已打开的窗口执行一次"
                    @click="runRule(rule)"
                  >
                    立即执行
                  </button>
                  <button type="button" class="link-btn danger" @click="removeRule(rule)">
                    删除
                  </button>
                  <Switch
                    size="small"
                    :checked="rule.enabled"
                    @change="(checked) => toggleRule(rule, checked === true)"
                  />
                </div>
              </div>
            </div>
          </template>
        </div>

        <div v-if="popup.ignoredProcesses.length" class="ignored">
          <span class="ignored-label">忽略名单</span>
          <span v-for="process in popup.ignoredProcesses" :key="process" class="ignored-chip">
            {{ process }}
            <button type="button" title="移出忽略名单" @click="setIgnore(process, false)">×</button>
          </span>
          <span class="ignored-hint">这些程序的窗口不会被检测，也不会被拦截</span>
        </div>
      </template>

      <!-- 窗口记录：没命中规则、但看起来像广告的窗口，一闪而过也能在这里找到 -->
      <template v-else>
        <p class="records-hint">弹窗一闪而过？试试在这里找到它</p>
        <div v-if="!popup.pendingHits.length" class="empty">
          <PopupShield :size="56" :active="popup.running" />
          <div class="empty-title">
            {{ popup.running ? '暂时没有窗口记录' : '开启后，应用弹窗会记录在这里' }}
          </div>
          <p class="empty-desc">
            {{
              popup.running
                ? '没有命中规则、但特征可疑的窗口会先进这里；一闪而过的广告窗也能在这里找到，点「拦截此类」生成规则。'
                : '检测只在 Windows 上生效，并且只能处理已经显示出来的窗口。'
            }}
          </p>
        </div>
        <div v-else class="list">
          <div v-for="hit in popup.pendingHits" :key="hit.id" class="row">
            <AppIcon
              :name="popup.displayName(hit.path, hit.process)"
              :icon="popup.iconOf(hit.path)"
              :size="30"
            />
            <div class="row-main">
              <div class="row-title" :title="hit.title">
                {{ hit.title || '（无标题窗口）' }}
              </div>
              <div class="row-sub">
                <span class="row-process">{{ hit.process }}</span>
                · {{ hit.class_name || '无类名' }} · {{ hit.width }}×{{ hit.height }} ·
                {{ hit.position || '任意位置' }}
                <span class="row-reasons" :title="hit.reasons.join('；')">
                  · {{ reasonText(hit) }}
                </span>
              </div>
            </div>
            <span v-if="hit.score > 0" class="score" :title="hit.reasons.join('；')">
              可疑 {{ hit.score }}
            </span>
            <span v-else class="tag tag-source" :title="hit.reasons.join('；')">规则未生效</span>
            <div class="row-actions">
              <button type="button" class="link-btn" @click="dismissHit(hit, 'close')">关闭</button>
              <button type="button" class="link-btn" @click="dismissHit(hit, 'hide')">隐藏</button>
              <button type="button" class="link-btn primary" @click="blockHit(hit)">拦截此类</button>
              <button type="button" class="link-btn" @click="ignoreHit(hit)">忽略</button>
            </div>
          </div>
        </div>
      </template>
    </div>

    <div class="config">
      <div class="config-item">
        <span class="config-label">默认动作</span>
        <Select
          size="small"
          style="width: 108px"
          :value="popup.settings.action"
          :options="ACTION_OPTIONS"
          @change="updateAction"
        />
      </div>
      <div class="config-item">
        <Switch
          size="small"
          :checked="popup.settings.smart"
          :disabled="!popup.supported"
          @change="toggleSmart"
        />
        <span class="config-label">智能拦截规则之外的弹窗</span>
        <template v-if="popup.settings.smart">
          <span class="config-muted">可疑分 ≥</span>
          <Select
            size="small"
            style="width: 64px"
            :value="popup.settings.min_score"
            :options="SCORE_OPTIONS"
            @change="setMinScore"
          />
        </template>
      </div>
      <span class="config-spacer" />
      <span class="config-muted">每分钟最多自动处理 {{ popup.settings.max_per_minute }} 个</span>
    </div>

    <p class="note">
      只处理应用弹出的独立广告 / 附属窗口，应用主窗口和系统窗口不会被碰；网页或界面内嵌的广告横幅
      需要在应用内关闭。拦截在窗口显示后进行（可能闪一下）；管理员权限窗口、UAC、锁屏处理不了。
      本工具不注入进程、不装驱动、不结束进程，只对已经显示出来的窗口发关闭 / 隐藏。
    </p>

    <PopupRuleDialog
      v-model:open="ruleDialogOpen"
      :target="ruleTarget"
      :action="popup.settings.action"
      :running="popup.running"
      :busy="popup.busy"
      :app-name="popup.displayName"
      :app-icon="popup.iconOf"
      @submit="submitRule"
    />
    <PopupWindowPicker
      v-model:open="pickerOpen"
      :windows="popup.windows"
      :loading="popup.windowsLoading"
      :app-name="popup.displayName"
      :app-icon="popup.iconOf"
      @refresh="popup.loadWindows"
      @pick="pickWindow"
    />
  </div>
</template>

<style scoped>
.btn-sm {
  padding: 3px 12px;
  font-size: 12px;
}

.popup {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px 20px 14px;
  overflow: hidden;
}

/* ---------------------------- 总开关 ---------------------------- */
.hero {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 14px 18px;
  border: 1px solid var(--border-color);
  border-radius: 12px;
  background: var(--bg-secondary);
}

.hero.on {
  border-color: rgba(59, 130, 246, 0.35);
  background: linear-gradient(90deg, rgba(59, 130, 246, 0.06), var(--bg-secondary) 60%);
}

.hero-text {
  flex: 1;
  min-width: 0;
}

.hero-title {
  display: flex;
  align-items: center;
  gap: 8px;
}

.hero-title h4 {
  color: var(--text-primary);
  font-size: 15px;
  font-weight: 600;
}

.hero-state {
  padding: 1px 7px;
  border-radius: 999px;
  background: var(--bg-tag);
  color: var(--text-secondary);
  font-size: 11px;
}

.hero-state.on {
  background: rgba(16, 185, 129, 0.12);
  color: var(--success);
}

.hero-hint {
  margin-top: 4px;
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1.6;
}

.hero-switch {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.hero-switch-label {
  color: var(--text-muted);
  font-size: 12px;
}

.hero-switch-label.on {
  color: var(--success);
}

/* ---------------------------- 统计 ---------------------------- */
.stats {
  flex-shrink: 0;
  display: flex;
  align-items: stretch;
  gap: 0;
  padding: 6px 6px;
  border: 1px solid var(--border-color);
  border-radius: 10px;
  background: var(--bg-primary);
}

.stat {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 1px;
  padding: 4px 8px;
  border-right: 1px solid var(--border-color);
}

.stat:last-child {
  border-right: none;
}

.stat b {
  color: var(--text-primary);
  font-size: 16px;
  font-variant-numeric: tabular-nums;
  line-height: 1.3;
}

.stat span {
  color: var(--text-muted);
  font-size: 11px;
}

.stat-wide {
  flex-direction: row;
  gap: 8px;
}

.stat-time {
  font-size: 13px !important;
  font-weight: 500;
}

.stat-muted {
  color: var(--text-muted);
}

/* ---------------------------- 分段页签 ---------------------------- */
.subbar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.segmented {
  display: inline-flex;
  gap: 2px;
  padding: 2px;
  border-radius: 8px;
  background: var(--bg-hover);
}

.segmented-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 12px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
}

.segmented-item:hover {
  color: var(--text-primary);
}

.segmented-item.active {
  background: var(--bg-primary);
  color: var(--text-primary);
  font-weight: 600;
  box-shadow: 0 1px 3px rgba(15, 23, 42, 0.08);
}

.segmented-count {
  min-width: 16px;
  padding: 0 4px;
  border-radius: 8px;
  background: var(--bg-tag);
  color: var(--text-secondary);
  font-size: 10px;
  font-weight: 400;
  line-height: 16px;
  text-align: center;
}

.segmented-item.active .segmented-count {
  background: var(--bg-active);
  color: var(--primary);
}

.subbar-actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}

/* ---------------------------- 规则表（火绒式按软件分组） ---------------------------- */
.rules-toolbar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 8px 14px;
  border-bottom: 1px solid var(--border-color);
  background: var(--bg-primary);
}

.records-hint {
  flex-shrink: 0;
  margin: 0;
  padding: 8px 14px;
  border-bottom: 1px solid var(--border-color);
  color: var(--text-muted);
  font-size: 11px;
}

.rules-summary {
  color: var(--text-muted);
  font-size: 11px;
}

.rule-table {
  display: flex;
  flex-direction: column;
}

.rule-head,
.app-row,
.rule-line {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 96px 88px 150px;
  align-items: center;
  gap: 10px;
  padding: 9px 14px;
}

.rule-head {
  position: sticky;
  top: 0;
  z-index: 2;
  border-bottom: 1px solid var(--border-color);
  background: var(--bg-secondary);
  color: var(--text-muted);
  font-size: 11px;
}

.rule-head span:nth-child(3),
.rule-head span:nth-child(4) {
  text-align: right;
}

.app-row {
  border-bottom: 1px solid var(--border-color);
  cursor: pointer;
}

.app-row:hover {
  background: var(--bg-hover);
}

.app-cell {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.app-main {
  flex: 1;
  min-width: 0;
}

.app-name {
  display: flex;
  align-items: baseline;
  gap: 8px;
  overflow: hidden;
  color: var(--text-primary);
  font-size: 13px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.app-process {
  color: var(--text-muted);
  font-size: 10px;
  font-weight: 400;
}

.app-sub {
  margin-top: 2px;
  color: var(--text-muted);
  font-size: 11px;
}

.chevron {
  flex-shrink: 0;
  width: 14px;
  height: 14px;
  fill: none;
  stroke: var(--text-secondary);
  stroke-linecap: round;
  stroke-linejoin: round;
  stroke-width: 2;
  transition: transform 0.15s;
}

.chevron.open {
  transform: rotate(180deg);
}

.cell-muted {
  overflow: hidden;
  color: var(--text-muted);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cell-hits {
  color: var(--text-secondary);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  text-align: right;
}

.cell-op {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  flex-shrink: 0;
}

.rule-lines {
  border-bottom: 1px solid var(--border-color);
  background: var(--bg-secondary);
}

.rule-line {
  padding-top: 7px;
  padding-bottom: 7px;
  border-bottom: 1px solid var(--border-color);
}

.rule-line:last-child {
  border-bottom: none;
}

.rule-line:hover {
  background: var(--bg-hover);
}

.rule-name-cell {
  min-width: 0;
  padding-left: 36px;
}

.rule-name {
  overflow: hidden;
  color: var(--text-primary);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.rule-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 2px;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 10px;
  white-space: nowrap;
}

.rule-action {
  flex-shrink: 0;
  color: var(--text-muted);
}

/* ---------------------------- 列表 ---------------------------- */
.body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow-y: auto;
  border: 1px solid var(--border-color);
  border-radius: 10px;
  background: var(--bg-primary);
}

.list {
  display: flex;
  flex-direction: column;
}

.row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 14px;
  border-bottom: 1px solid var(--border-color);
}

.row:last-child {
  border-bottom: none;
}

.row:hover {
  background: var(--bg-hover);
}

.row-main {
  flex: 1;
  min-width: 0;
}

.row-title {
  color: var(--text-primary);
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row-sub {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 3px;
  color: var(--text-muted);
  font-size: 11px;
  overflow: hidden;
  white-space: nowrap;
}

.row-process {
  flex-shrink: 0;
  color: var(--text-secondary);
}

.row-reasons {
  overflow: hidden;
  text-overflow: ellipsis;
}

.row-time {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}

.rule-detail {
  overflow: hidden;
  text-overflow: ellipsis;
}

.rule-hits {
  flex-shrink: 0;
  color: var(--text-muted);
}

.row-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.rule-row .row-actions {
  gap: 12px;
}

.tag {
  flex-shrink: 0;
  padding: 1px 7px;
  border-radius: 4px;
  font-size: 11px;
  line-height: 18px;
}

.tag-block {
  background: rgba(16, 185, 129, 0.12);
  color: var(--success);
}

.tag-hide {
  background: rgba(59, 130, 246, 0.12);
  color: var(--primary);
}

.tag-source {
  background: var(--bg-hover);
  color: var(--text-secondary);
}

.tag-scope {
  background: var(--bg-hover);
  color: var(--text-secondary);
}

.score {
  flex-shrink: 0;
  padding: 2px 8px;
  border-radius: 10px;
  background: rgba(245, 158, 11, 0.12);
  color: #b45309;
  font-size: 11px;
  font-weight: 600;
}

.link-btn {
  padding: 0;
  border: none;
  background: none;
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
}

.link-btn:hover {
  color: var(--primary);
}

.link-btn:disabled {
  color: var(--text-muted);
  cursor: default;
}

.link-btn:disabled:hover {
  color: var(--text-muted);
}

.link-btn.primary {
  color: var(--primary);
  font-weight: 600;
}

.link-btn.danger:hover {
  color: var(--danger);
}

/* ---------------------------- 空状态 ---------------------------- */
.empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 36px 24px;
  text-align: center;
}

.empty-title {
  color: var(--text-secondary);
  font-size: 13px;
  font-weight: 500;
}

.empty-desc {
  max-width: 460px;
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1.7;
}

.empty-actions {
  display: flex;
  gap: 8px;
  margin-top: 6px;
}

/* ---------------------------- 忽略名单 ---------------------------- */
.ignored {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  margin: 0 14px 12px;
  padding: 10px 12px;
  border: 1px dashed var(--border-color);
  border-radius: 8px;
  background: var(--bg-secondary);
}

.ignored-label {
  color: var(--text-secondary);
  font-size: 11px;
}

.ignored-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 1px 4px 1px 8px;
  border-radius: 999px;
  background: var(--bg-primary);
  border: 1px solid var(--border-color);
  color: var(--text-secondary);
  font-size: 11px;
}

.ignored-chip button {
  width: 16px;
  height: 16px;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1;
  cursor: pointer;
}

.ignored-chip button:hover {
  background: var(--bg-hover);
  color: var(--danger);
}

.ignored-hint {
  color: var(--text-muted);
  font-size: 11px;
}

/* ---------------------------- 设置条 / 说明 ---------------------------- */
.config {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 10px 18px;
  padding: 8px 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-secondary);
}

.config-item {
  display: flex;
  align-items: center;
  gap: 8px;
}

.config-label {
  color: var(--text-secondary);
  font-size: 12px;
}

.config-muted {
  color: var(--text-muted);
  font-size: 11px;
}

.config-spacer {
  flex: 1;
}

.note {
  flex-shrink: 0;
  margin: 0;
  color: var(--text-muted);
  font-size: 11px;
  line-height: 1.6;
}

/* ---------------------------- 错误 ---------------------------- */
.popup-error {
  flex-shrink: 0;
  margin: 0;
  padding: 8px 10px;
  border-radius: 6px;
  background: rgba(239, 68, 68, 0.08);
  color: var(--danger);
  font-size: 12px;
}
</style>
