<script setup lang="ts">
import { computed, ref } from 'vue'
import { message } from 'ant-design-vue'
import {
  setLanShareDownloadDir,
  setLanShareEnabled,
  type LanShareSnapshot,
} from '@/api/lanshare'
import { openDir } from '@/api/system'
import { pickDirectory } from '@/utils/dialog'
import LanAliasEditor from './LanAliasEditor.vue'
import LanRequireAccept from './LanRequireAccept.vue'
import LanSoundSetting from './LanSoundSetting.vue'

/**
 * 右栏设置页：把改造前顶部「本机信息卡」的内容搬进来。
 *
 * 设备名编辑拆在 LanAliasEditor（保留原来「只有 ✓ / Enter 才保存」的逻辑）；
 * 每个命令都返回最新快照，这里 apply 后上抛给壳，保证左右栏状态一致。
 */
const props = defineProps<{
  snapshot: LanShareSnapshot | null
}>()

const emit = defineEmits<{
  close: []
  updated: [snapshot: LanShareSnapshot]
}>()

const toggling = ref(false)
const savingDir = ref(false)

const enabled = computed(() => props.snapshot?.enabled ?? false)
const running = computed(() => props.snapshot?.running ?? false)
const statusText = computed(() => {
  if (!props.snapshot) return '读取中…'
  if (props.snapshot.last_error) return '启动失败'
  if (running.value) return '运行中'
  if (enabled.value) return '启动中…'
  return '未开启'
})
const statusClass = computed(() => {
  if (!props.snapshot) return 'is-off'
  if (props.snapshot.last_error) return 'is-error'
  if (running.value) return 'is-on'
  if (enabled.value) return 'is-busy'
  return 'is-off'
})
const localAddresses = computed(() => (props.snapshot?.local_addresses ?? []).join(' · '))
/** 身份是主网卡 MAC 时完整显示；取不到 MAC 回退 UUID 时按设计只给短指纹（title 仍显示全量） */
const MAC_RE = /^([0-9a-f]{2}:){5}[0-9a-f]{2}$/i
const deviceIdFull = computed(() => props.snapshot?.fingerprint ?? '')
const deviceIdText = computed(() => {
  const id = deviceIdFull.value
  if (!id) return '—'
  if (MAC_RE.test(id)) return id
  return id.length > 12 ? `${id.slice(0, 8)}…${id.slice(-4)}` : id
})

function errorText(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  return String(err)
}

function apply(next: LanShareSnapshot) {
  emit('updated', next)
}

async function toggleEnabled() {
  if (!props.snapshot || toggling.value) return
  toggling.value = true
  const next = !enabled.value
  try {
    apply(await setLanShareEnabled(next))
    message.success(next ? '局域网分享已开启' : '局域网分享已关闭')
  } catch (err) {
    message.error(`切换失败：${errorText(err)}`)
  } finally {
    toggling.value = false
  }
}

async function chooseDownloadDir() {
  if (!props.snapshot || savingDir.value) return
  let dir: string | null = null
  try {
    dir = await pickDirectory({
      defaultPath: props.snapshot.download_dir || undefined,
      title: '选择接收文件的保存目录',
    })
  } catch (err) {
    message.error(`打开目录选择框失败：${errorText(err)}`)
    return
  }
  if (!dir) return
  savingDir.value = true
  try {
    apply(await setLanShareDownloadDir(dir))
    message.success('接收目录已更新')
  } catch (err) {
    message.error(`设置接收目录失败：${errorText(err)}`)
  } finally {
    savingDir.value = false
  }
}

async function resetDownloadDir() {
  if (!props.snapshot || savingDir.value) return
  savingDir.value = true
  try {
    apply(await setLanShareDownloadDir(null))
    message.success('已恢复默认接收目录')
  } catch (err) {
    message.error(`恢复默认目录失败：${errorText(err)}`)
  } finally {
    savingDir.value = false
  }
}

async function openDownloadDir() {
  if (!props.snapshot?.download_dir) return
  try {
    await openDir(props.snapshot.download_dir)
  } catch (err) {
    message.error(`打开目录失败：${errorText(err)}`)
  }
}
</script>

<template>
  <section class="settings-pane">
    <header class="settings-head">
      <h3>设置</h3>
      <button class="back-btn" type="button" @click="emit('close')">返回聊天</button>
    </header>

    <div class="settings-body">
      <p v-if="!snapshot" class="settings-empty">正在读取局域网分享状态…</p>

      <template v-else>
        <p v-if="snapshot.last_error" class="alert is-error">
          启动失败：{{ snapshot.last_error }}
          <span class="alert-hint">请检查端口 / 防火墙设置后重新开启。</span>
        </p>
        <!-- 服务在跑、只是设置有改动没存住：跟「启动失败」分开显示，别吓人 -->
        <p v-if="snapshot.warning" class="alert is-warn">{{ snapshot.warning }}</p>

        <div class="settings-card">
          <!-- 设备名就地编辑 + 运行状态 + 开关 -->
          <div class="alias-row">
            <LanAliasEditor :alias="snapshot.alias" @updated="emit('updated', $event)" />
            <div class="local-actions">
              <span class="lan-status" :class="statusClass">
                <span class="lan-status-dot"></span>
                {{ statusText }}
                <template v-if="running && snapshot.port">（端口 {{ snapshot.port }}）</template>
              </span>
              <button
                class="btn btn-xs"
                :class="enabled ? 'btn-danger-ghost' : 'btn-primary'"
                type="button"
                :disabled="!snapshot || toggling"
                @click="toggleEnabled"
              >
                {{ toggling ? '处理中…' : enabled ? '关闭功能' : '开启功能' }}
              </button>
            </div>
          </div>

          <dl class="info-list">
            <div class="info-row">
              <dt>监听端口</dt>
              <dd class="mono">{{ snapshot.port ?? '—' }}</dd>
            </div>
            <div class="info-row">
              <dt>本机地址</dt>
              <dd class="mono info-wrap">
                {{ localAddresses || '未获取到可用网卡地址' }}
              </dd>
            </div>
            <div class="info-row">
              <dt>设备 ID（MAC）</dt>
              <!-- MAC 完整展示；回退 UUID 时短指纹 + 悬浮看全量 -->
              <dd class="mono info-wrap" :title="deviceIdFull">{{ deviceIdText }}</dd>
            </div>
            <div class="info-row dir-row">
              <dt>接收目录</dt>
              <dd class="dir-value">
                <span class="dir-path mono" :title="snapshot.download_dir || ''">
                  {{ snapshot.download_dir || '—' }}
                </span>
                <span class="dir-actions">
                  <button
                    class="text-btn"
                    type="button"
                    :disabled="savingDir"
                    @click="chooseDownloadDir"
                  >
                    选择目录
                  </button>
                  <button class="text-btn" type="button" @click="openDownloadDir">打开目录</button>
                  <button
                    v-if="snapshot.download_dir_custom"
                    class="text-btn"
                    type="button"
                    :disabled="savingDir"
                    @click="resetDownloadDir"
                  >
                    恢复默认
                  </button>
                </span>
              </dd>
            </div>
          </dl>
        </div>

        <!-- 接收前确认：默认不勾选（直接接收），勾选后恢复「待接收」流程 -->
        <LanRequireAccept :snapshot="snapshot" @updated="emit('updated', $event)" />

        <!-- 消息提示音：和左栏右键「免打扰模式」共享同一份状态 -->
        <LanSoundSetting />

        <p class="settings-tip">
          设备 ID 使用主物理网卡 MAC，跨重启 / 换 IP 不变。同一局域网内打开本应用的设备会出现在左侧列表，
          点击即可聊天或发送文件。
        </p>
      </template>
    </div>
  </section>
</template>

<style scoped>
.settings-pane {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.settings-head {
  flex-shrink: 0;
  height: var(--panel-header-height, 40px);
  padding: 0 16px;
  display: flex;
  align-items: center;
  gap: 12px;
  border-bottom: 1px solid var(--border-color);
}
.settings-head h3 {
  flex: 1;
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
  margin: 0;
}
.back-btn {
  padding: 4px 10px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-input);
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
  transition: border-color 0.15s, color 0.15s;
}
.back-btn:hover {
  border-color: var(--primary);
  color: var(--primary);
}

.settings-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 16px 20px 24px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.settings-empty {
  font-size: 13px;
  color: var(--text-muted);
  line-height: 1.6;
}

.alert {
  margin: 0;
  padding: 8px 12px;
  border-radius: 6px;
  font-size: 12px;
  line-height: 1.6;
}
.alert.is-error {
  border: 1px solid rgba(239, 68, 68, 0.3);
  background: rgba(239, 68, 68, 0.06);
  color: var(--danger);
}
.alert.is-warn {
  border: 1px solid rgba(245, 158, 11, 0.32);
  background: rgba(245, 158, 11, 0.08);
  color: #b45309;
}
.alert-hint {
  color: var(--text-muted);
}

.settings-card {
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  padding: 14px;
}

/* 设备名 + 状态 + 开关 */
.alias-row {
  display: flex;
  align-items: center;
  gap: 16px;
  padding-bottom: 12px;
  margin-bottom: 12px;
  border-bottom: 1px dashed var(--border-color);
}
.local-actions {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 10px;
}
.lan-status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
}
.lan-status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-muted);
  flex-shrink: 0;
}
.lan-status.is-on {
  color: var(--success);
}
.lan-status.is-on .lan-status-dot {
  background: var(--success);
  box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.16);
}
.lan-status.is-busy {
  color: var(--warning);
}
.lan-status.is-busy .lan-status-dot {
  background: var(--warning);
}
.lan-status.is-error {
  color: var(--danger);
}
.lan-status.is-error .lan-status-dot {
  background: var(--danger);
}
.btn.btn-xs {
  padding: 4px 10px;
  font-size: 12px;
  flex-shrink: 0;
}

/* 本机信息：标签定宽，值可换行 */
.info-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.info-row {
  display: flex;
  align-items: baseline;
  gap: 12px;
  font-size: 12px;
  min-height: 24px;
}
.info-row dt {
  width: 96px;
  flex-shrink: 0;
  color: var(--text-muted);
}
.info-row dd {
  flex: 1;
  min-width: 0;
  color: var(--text-secondary);
}
.info-wrap {
  word-break: break-all;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}

/* 接收目录：路径后面紧跟文字按钮，路径太长时优先省略路径 */
.dir-row {
  align-items: center;
}
.dir-value {
  display: flex;
  align-items: center;
  gap: 6px;
}
.dir-path {
  flex: 0 1 auto;
  min-width: 0;
  font-size: 12px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.dir-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
}
.text-btn {
  padding: 2px 6px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  line-height: 1.5;
  cursor: pointer;
  white-space: nowrap;
  transition: background 0.15s, color 0.15s;
}
.text-btn:hover:not(:disabled) {
  background: var(--bg-hover);
}
.text-btn:disabled {
  color: var(--text-muted);
  cursor: not-allowed;
}

.settings-tip {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--text-muted);
}

/* 窄窗口：设备名行上下排，别把开关挤出去 */
@media (max-width: 760px) {
  .alias-row {
    flex-direction: column;
    align-items: stretch;
    gap: 10px;
  }
  .local-actions {
    justify-content: flex-end;
  }
}
</style>
