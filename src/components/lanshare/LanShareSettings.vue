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
 * 内容以白色为主：一张带细边框的设置卡，设备名/接收目录/本机地址/设备 ID
 * 用「左标签 + 右值」的同一套行结构，靠分隔线分行；启动失败 / 设置未存住仍用顶部告警展示。
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
        <div class="settings-content">
          <p v-if="snapshot.last_error" class="alert is-error">
            启动失败：{{ snapshot.last_error }}
            <span class="alert-hint">请检查端口 / 防火墙设置后重启应用。</span>
          </p>
          <!-- 服务在跑、只是设置有改动没存住：跟「启动失败」分开显示，别吓人 -->
          <p v-if="snapshot.warning" class="alert is-warn">{{ snapshot.warning }}</p>

          <header class="settings-head">
            <h2 class="settings-title">本机设置</h2>
            <p class="settings-sub">设备身份、接收位置与网络信息</p>
          </header>

          <!-- 所有设置收在同一张白色卡片里，统一「左标签 + 右值」的行结构 -->
          <div class="settings-card">
            <!-- 本机设备名 + 监听端口（端口不可关闭，跟在名字后面） -->
            <div class="setting-row alias-row">
              <LanAliasEditor :alias="snapshot.alias" @updated="emit('updated', $event)" />
              <span class="port-tag" title="应用启动即监听，不可单独关闭">
                · {{ snapshot.port ?? '—' }}
              </span>
            </div>

            <!-- 接收目录：路径 + 轻量文字按钮 -->
            <div class="setting-row">
              <span class="setting-label">接收目录</span>
              <div class="setting-value">
                <div class="dir-line">
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
                    <button class="text-btn" type="button" @click="openDownloadDir">
                      打开目录
                    </button>
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
                </div>
              </div>
            </div>

            <div class="setting-row">
              <span class="setting-label">本机地址</span>
              <p class="setting-value mono">{{ localAddresses || '未获取到可用网卡地址' }}</p>
            </div>

            <div class="setting-row">
              <span class="setting-label">设备 ID</span>
              <!-- MAC 完整展示；回退 UUID 时短指纹 + 悬浮看全量 -->
              <p class="setting-value mono" :title="deviceIdFull">{{ deviceIdText }}</p>
            </div>
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
  background: #fff;
}

.settings-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 24px 28px 32px;
}
.settings-content {
  width: 100%;
  max-width: 860px;
  margin: 0 auto;
}
.settings-empty {
  margin: 0;
  font-size: 13px;
  color: var(--text-muted);
  line-height: 1.6;
}

/* 告警：只做提示，不抢设置卡的层次 */
.alert {
  margin: 0 0 12px;
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

.settings-head {
  padding: 2px 4px 14px;
}
.settings-title {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
  color: var(--text-primary);
}
.settings-sub {
  margin: 4px 0 0;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}

/* 白底设置卡：细边框 + 很轻的阴影，和纯白页面的层次拉开一点即可 */
.settings-card {
  padding: 4px 20px;
  border: 1px solid #eef0f4;
  border-radius: 14px;
  background: #fff;
  box-shadow: 0 1px 2px rgba(15, 23, 42, 0.03);
}
.setting-row {
  display: flex;
  align-items: flex-start;
  gap: 16px;
  padding: 15px 0;
}
.setting-row + .setting-row {
  border-top: 1px solid #f0f2f5;
}
.setting-label {
  flex-shrink: 0;
  width: 92px;
  padding-top: 1px;
  font-size: 13px;
  line-height: 1.6;
  color: var(--text-muted);
}
.setting-value {
  flex: 1;
  min-width: 0;
  margin: 0;
  font-size: 13px;
  line-height: 1.6;
  color: var(--text-secondary);
  word-break: break-all;
}

/* 设备名行：LanAliasEditor 自带标签，用 :deep 对齐其它行的标签列 */
.alias-row {
  align-items: center;
}
.alias-row :deep(.alias-editor) {
  flex: 1;
  gap: 16px;
}
.alias-row :deep(.alias-label) {
  width: 92px;
  flex-shrink: 0;
  font-size: 13px;
  line-height: 1.6;
}
.alias-row :deep(.alias-name) {
  font-size: 14px;
}
.port-tag {
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-muted);
  white-space: nowrap;
}

/* 接收目录：路径优先占宽，按钮不换行 */
.dir-line {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
  flex-wrap: wrap;
  row-gap: 4px;
}
.dir-path {
  flex: 1 1 160px;
  min-width: 0;
  font-size: 12px;
  line-height: 1.6;
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

@media (max-width: 760px) {
  .settings-body {
    padding: 18px 16px 24px;
  }
  .settings-card {
    padding: 2px 16px;
  }
  .setting-label,
  .alias-row :deep(.alias-label) {
    width: 76px;
  }
  .dir-path {
    flex-basis: 120px;
  }
}
</style>
