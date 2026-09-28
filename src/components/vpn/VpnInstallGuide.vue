<script setup lang="ts">
import type { InstallGuide } from '@/types/vpn'

defineProps<{
  guide: InstallGuide | null
  /** 是否允许「一键导入 openconnect」 */
  canImport: boolean
}>()

const emit = defineEmits<{
  import: []
  'copy-command': [command: string]
}>()

async function copyCommand(command: string) {
  try {
    await navigator.clipboard.writeText(command)
    emit('copy-command', command)
  } catch {
    emit('copy-command', '复制失败，请手动选中命令复制')
  }
}
</script>

<template>
  <div v-if="guide && guide.steps.length > 0" class="guide">
    <div class="guide-head">
      <span class="guide-dot"></span>
      <div>
        <h4>{{ guide.title }}</h4>
        <p>{{ guide.description }}</p>
      </div>
    </div>

    <ol class="guide-steps">
      <li v-for="(step, index) in guide.steps" :key="index">
        <div class="step-title">{{ step.title }}</div>
        <p v-if="step.detail" class="step-detail">{{ step.detail }}</p>
        <div v-if="step.command" class="step-command">
          <code>{{ step.command }}</code>
          <button class="btn-copy" @click="copyCommand(step.command)">复制</button>
        </div>
      </li>
    </ol>

    <div v-if="canImport" class="guide-import">
      <div class="import-text">
        <strong>已经有 openconnect 了？</strong>
        <span>直接选它所在的那个目录，程序会把 openconnect.exe 和同目录的 DLL 复制到自己的数据目录里，不需要管理员权限。</span>
      </div>
      <button class="btn btn-primary" @click="emit('import')">一键导入</button>
    </div>

    <div v-if="guide.links.length > 0" class="guide-links">
      <a
        v-for="link in guide.links"
        :key="link.url"
        :href="link.url"
        target="_blank"
        rel="noreferrer"
      >
        {{ link.label }} ↗
      </a>
    </div>
  </div>
</template>

<style scoped>
.guide {
  border: 1px solid rgba(245, 158, 11, 0.35);
  background: rgba(245, 158, 11, 0.06);
  border-radius: 8px;
  padding: 16px;
}
.guide-head {
  display: flex;
  gap: 10px;
  align-items: flex-start;
}
.guide-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--warning);
  margin-top: 6px;
  flex-shrink: 0;
}
.guide-head h4 {
  font-size: 14px;
  color: var(--text-primary);
  margin-bottom: 4px;
}
.guide-head p {
  font-size: 13px;
  line-height: 1.6;
  color: var(--text-secondary);
}
.guide-steps {
  margin: 14px 0 0 18px;
  padding: 0;
}
.guide-steps li {
  margin-bottom: 12px;
  font-size: 13px;
  color: var(--text-secondary);
}
.step-title {
  font-weight: 500;
  color: var(--text-primary);
}
.step-detail {
  margin-top: 4px;
  line-height: 1.6;
}
.step-command {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 6px;
}
.step-command code {
  flex: 1;
  background: var(--bg-tag);
  border-radius: 4px;
  padding: 5px 8px;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  color: var(--text-primary);
  word-break: break-all;
}
.btn-copy {
  border: 1px solid var(--border-color);
  background: var(--bg-input);
  border-radius: 4px;
  padding: 3px 10px;
  font-size: 12px;
  cursor: pointer;
  color: var(--text-secondary);
  flex-shrink: 0;
}
.btn-copy:hover {
  border-color: var(--primary);
  color: var(--primary);
}
.guide-import {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 16px;
  padding-top: 14px;
  border-top: 1px dashed rgba(245, 158, 11, 0.35);
}
.import-text {
  display: flex;
  flex-direction: column;
  gap: 3px;
  font-size: 12px;
  color: var(--text-secondary);
  line-height: 1.55;
}
.import-text strong {
  font-size: 13px;
  color: var(--text-primary);
}
.guide-links {
  display: flex;
  flex-wrap: wrap;
  gap: 14px;
  margin-top: 14px;
  font-size: 12px;
}
.guide-links a {
  color: var(--primary);
  text-decoration: none;
}
.guide-links a:hover {
  text-decoration: underline;
}
</style>
