<script setup lang="ts">
import { computed, ref } from 'vue'
import { message } from 'ant-design-vue'
import { setLanShareDownloadDir, type LanShareSnapshot } from '@/api/lanshare'
import { openDir } from '@/api/system'
import { pickDirectory } from '@/utils/dialog'
import LanAliasEditor from './LanAliasEditor.vue'

/**
 * 右栏设置页（LANLink 刷新 §1 / §2.2）。
 *
 * 不再有标题和「返回聊天」：设置由左栏齿轮进入、再点一次齿轮或选设备返回。
 * 内容是一张白色悬浮卡片，本机设备名（端口跟在名字后）、接收目录、
 * 本机地址、设备 ID 合在同一块里，用细分隔线分行；启动失败 / 设置未存住仍用顶部告警展示。
 *
 * 设备名编辑拆在 LanAliasEditor（只有 ✓ / Enter 才保存）；命令返回最新快照，
 * 这里 apply 后上抛给壳，保证左右栏状态一致。
 */
const props = defineProps<{
  snapshot: LanShareSnapshot | null
}>()

const emit = defineEmits<{
  updated: [snapshot: LanShareSnapshot]
}>()

const savingDir = ref(false)

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
    emit('updated', await setLanShareDownloadDir(dir))
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
    emit('updated', await setLanShareDownloadDir(null))
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
    <div class="settings-body">
      <p v-if="!snapshot" class="settings-empty">正在读取 LANLink 状态…</p>

      <template v-else>
        <p v-if="snapshot.last_error" class="alert is-error">
          启动失败：{{ snapshot.last_error }}
          <span class="alert-hint">请检查端口 / 防火墙设置后重启应用。</span>
        </p>
        <!-- 服务在跑、只是设置有改动没存住：跟「启动失败」分开显示，别吓人 -->
        <p v-if="snapshot.warning" class="alert is-warn">{{ snapshot.warning }}</p>

        <!-- 所有设置收在同一张卡片里，行间用细分隔线，避免一块一块浮起 -->
        <div class="card settings-card">
          <!-- 本机设备名 + 监听端口（端口不可关闭，跟在名字后面） -->
          <div class="setting-block alias-block">
            <LanAliasEditor :alias="snapshot.alias" @updated="emit('updated', $event)" />
            <span class="port-tag" title="应用启动即监听，不可单独关闭">
              · {{ snapshot.port ?? '—' }}
            </span>
          </div>

          <!-- 接收目录：路径 + 轻量文字按钮 -->
          <div class="setting-block">
            <span class="card-title">接收目录</span>
            <p class="dir-value">
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
            </p>
          </div>

          <div class="setting-block">
            <span class="card-title">本机地址</span>
            <p class="card-value mono">{{ localAddresses || '未获取到可用网卡地址' }}</p>
          </div>

          <div class="setting-block">
            <span class="card-title">设备 ID</span>
            <!-- MAC 完整展示；回退 UUID 时短指纹 + 悬浮看全量 -->
            <p class="card-value mono" :title="deviceIdFull">{{ deviceIdText }}</p>
          </div>
        </div>
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

/* 无标题、无返回：内容直接铺开滚动 */
.settings-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 18px 20px 24px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.settings-empty {
  margin: 0;
  font-size: 13px;
  color: var(--text-muted);
  line-height: 1.6;
}

/* 告警：只做提示，不抢卡片层次 */
.alert {
  margin: 0;
  padding: 10px 14px;
  border-radius: 12px;
  font-size: 12px;
  line-height: 1.6;
}
.alert.is-error {
  background: rgba(239, 68, 68, 0.08);
  color: var(--danger);
}
.alert.is-warn {
  background: rgba(245, 158, 11, 0.1);
  color: #b45309;
}
.alert-hint {
  color: var(--text-muted);
}

/* 悬浮卡片：唯一 elevation 体系里的低/中阴影，不用边框 */
.card {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 16px 18px;
  border-radius: 14px;
  background: #fff;
  box-shadow:
    0 1px 2px rgba(15, 23, 42, 0.04),
    0 10px 30px -18px rgba(15, 23, 42, 0.28);
}

/* 所有设置共用一张卡片：行内不再各自留白，靠分隔线区分 */
.settings-card {
  gap: 0;
  padding: 2px 18px;
}
.setting-block {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 14px 0;
}
.setting-block + .setting-block {
  border-top: 1px solid var(--border-color);
}
.card-title {
  font-size: 12px;
  color: var(--text-muted);
  letter-spacing: 0.02em;
}
.card-value {
  margin: 0;
  font-size: 13px;
  line-height: 1.6;
  color: var(--text-secondary);
  word-break: break-all;
}

/* 设备名 + 端口同一行 */
.alias-block {
  flex-direction: row;
  align-items: center;
  gap: 10px;
}
.port-tag {
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-muted);
  white-space: nowrap;
}

/* 接收目录：路径太长时省略，按钮不换行 */
.dir-value {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0;
  min-width: 0;
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
  border-radius: 6px;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  line-height: 1.5;
  cursor: pointer;
  white-space: nowrap;
  transition: background 0.16s, color 0.16s;
}
.text-btn:hover:not(:disabled) {
  background: var(--bg-hover);
}
.text-btn:disabled {
  color: var(--text-muted);
  cursor: not-allowed;
}

.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}

/* 窄窗口：设备名行允许端口换到下一行 */
@media (max-width: 760px) {
  .alias-block {
    flex-wrap: wrap;
  }
}
</style>
