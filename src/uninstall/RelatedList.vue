<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import AppIcon from '@/components/cleaner/AppIcon.vue'
import { formatBytes } from '@/components/cleaner/format'
import type { RelatedFileGroup, RelatedFileItem } from '@/types/cleaner'

/**
 * 关联文件清单：程序文件 / 支持文件 / 缓存文件三组。
 *
 * - 组头勾选框三态（全选 / 部分 / 未选），只改该组；
 * - 点组名或箭头折叠该组；
 * - 条目：勾选框 + 图标 + 名称 + 版本徽标（仅 app_bundle）+ 右对齐大小。
 *
 * 勾选状态由父组件持有（删除时需要按 path 汇总），这里用 `update:selected` 回传。
 */
const props = defineProps<{
  groups: RelatedFileGroup[]
  /** path -> 是否勾选；只有 true 的键存在 */
  selected: Record<string, boolean>
  /** 应用图标 data URL，程序文件组条目复用 */
  appIcon?: string | null
  /** 应用名，图标兜底用 */
  appName: string
  /**
   * 应用本体行路径：这些行代表「系统卸载」的目标，必须始终勾选且不可取消
   * （macOS 是安装路径的 .app，Windows 是 install_dir）。
   */
  primaryPaths?: string[]
}>()

const emit = defineEmits<{
  (event: 'update:selected', value: Record<string, boolean>): void
}>()

/** group id -> 是否折叠；换目标重新扫描时重置 */
const collapsed = reactive<Record<string, boolean>>({})

/** 空组不渲染（后端仍可能返回 0 项的分组） */
const visibleGroups = computed(() => props.groups.filter((group) => group.items.length > 0))

watch(
  () => props.groups,
  () => {
    for (const key of Object.keys(collapsed)) delete collapsed[key]
  },
)

/** 应用本体行：勾选框锁定为已选（确认删除一定会执行系统卸载） */
function isPrimary(item: RelatedFileItem): boolean {
  return (props.primaryPaths ?? []).includes(item.path)
}

function isItemChecked(item: RelatedFileItem): boolean {
  return isPrimary(item) || !!props.selected[item.path]
}

function groupKey(group: RelatedFileGroup): string {
  return group.id || group.label
}

function isCollapsed(group: RelatedFileGroup): boolean {
  return !!collapsed[groupKey(group)]
}

function toggleCollapse(group: RelatedFileGroup): void {
  const key = groupKey(group)
  collapsed[key] = !collapsed[key]
}

function selectedCount(group: RelatedFileGroup): number {
  return group.items.filter((item) => isItemChecked(item)).length
}

function groupChecked(group: RelatedFileGroup): boolean {
  return group.items.length > 0 && selectedCount(group) === group.items.length
}

function groupIndeterminate(group: RelatedFileGroup): boolean {
  const count = selectedCount(group)
  return count > 0 && count < group.items.length
}

function toggleGroup(group: RelatedFileGroup, checked: boolean): void {
  const next = { ...props.selected }
  for (const item of group.items) {
    if (checked) next[item.path] = true
    // 本体行不可取消（父组件也会兜底补回）
    else if (!isPrimary(item)) delete next[item.path]
  }
  emit('update:selected', next)
}

function toggleItem(item: RelatedFileItem, checked: boolean): void {
  if (isPrimary(item)) return
  const next = { ...props.selected }
  if (checked) next[item.path] = true
  else delete next[item.path]
  emit('update:selected', next)
}

/** 程序文件里的 .app 用应用图标，其余用文件夹/文档/注册表小图标 */
function iconKind(item: RelatedFileItem): 'app' | 'registry' | 'doc' | 'folder' {
  if (item.kind === 'app_bundle') return 'app'
  if (item.kind === 'registry') return 'registry'
  if (!item.is_dir) return 'doc'
  return 'folder'
}
</script>

<template>
  <div class="related-scroll">
    <!-- 图标 sprite：三组清单共用 -->
    <svg class="related-sprite" aria-hidden="true" focusable="false">
      <defs>
        <linearGradient id="un-folder-grad" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#7cc4ff" />
          <stop offset="1" stop-color="#3f8ff0" />
        </linearGradient>
      </defs>
      <symbol id="un-folder" viewBox="0 0 20 20">
        <path
          d="M2 5.2c0-.9.72-1.6 1.6-1.6h4.06c.42 0 .83.17 1.13.47L10.3 5.6h6.1c.88 0 1.6.72 1.6 1.6v7.6c0 .88-.72 1.6-1.6 1.6H3.6c-.88 0-1.6-.72-1.6-1.6V5.2Z"
          fill="url(#un-folder-grad)"
        />
        <path d="M2 8h16v1.5H2z" fill="#ffffff" opacity="0.24" />
      </symbol>
      <symbol id="un-doc" viewBox="0 0 20 20">
        <path
          d="M5 2.5h6.4L15 6.1v11.4c0 .6-.4 1-1 1H5c-.6 0-1-.4-1-1v-14c0-.6.4-1 1-1Z"
          fill="#c9ccd1"
        />
        <path d="M11.2 2.5 15 6.3h-3.8V2.5Z" fill="#aeb2b8" />
        <path
          d="M6.4 10.2h6.2M6.4 13h6.2"
          stroke="#fff"
          stroke-width="1.2"
          stroke-linecap="round"
        />
      </symbol>
      <symbol id="un-registry" viewBox="0 0 20 20">
        <path
          d="M3 5.6c0-1.4 3.13-2.5 7-2.5s7 1.1 7 2.5v8.8c0 1.4-3.13 2.5-7 2.5s-7-1.1-7-2.5V5.6Z"
          fill="#c9ccd1"
        />
        <ellipse cx="10" cy="5.6" rx="7" ry="2.5" fill="#e2e5e9" />
        <path
          d="M3 10.2c1.3 1 4 1.6 7 1.6s5.7-.6 7-1.6"
          fill="none"
          stroke="#aeb2b8"
          stroke-width="1.1"
        />
      </symbol>
    </svg>

    <section v-for="group in visibleGroups" :key="groupKey(group)" class="related-group">
      <div class="related-group-head" @click="toggleCollapse(group)">
        <button
          type="button"
          class="u-check"
          :class="{ 'is-checked': groupChecked(group), 'is-mixed': groupIndeterminate(group) }"
          role="checkbox"
          :aria-checked="
            groupChecked(group) ? 'true' : groupIndeterminate(group) ? 'mixed' : 'false'
          "
          :aria-label="`选中${group.label}`"
          :disabled="group.items.length === 0"
          @click.stop="toggleGroup(group, !groupChecked(group))"
        >
          <svg v-if="groupChecked(group)" viewBox="0 0 14 14" aria-hidden="true">
            <path
              d="M3.2 7.5 5.9 10 10.8 4.4"
              fill="none"
              stroke="currentColor"
              stroke-width="1.9"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
          <span v-else-if="groupIndeterminate(group)" class="u-check-dash"></span>
        </button>
        <span class="related-group-label">{{ group.label }}</span>
        <svg
          class="related-chevron"
          :class="{ 'is-collapsed': isCollapsed(group) }"
          viewBox="0 0 12 8"
          aria-hidden="true"
        >
          <path
            d="M1.2 6.3 6 1.6l4.8 4.7"
            fill="none"
            stroke="currentColor"
            stroke-width="1.6"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      </div>

      <div v-if="!isCollapsed(group)" class="related-items">
        <div
          v-for="item in group.items"
          :key="item.path"
          class="related-row"
          :class="{ 'is-primary': isPrimary(item) }"
          :title="isPrimary(item) ? '卸载应用本体（必选）' : item.path"
        >
          <button
            type="button"
            class="u-check"
            :class="{ 'is-checked': isItemChecked(item), 'is-locked': isPrimary(item) }"
            role="checkbox"
            :aria-checked="isItemChecked(item) ? 'true' : 'false'"
            :aria-label="isPrimary(item) ? `卸载应用本体（必选）${item.name}` : `选中 ${item.name}`"
            :disabled="isPrimary(item)"
            @click="toggleItem(item, !isItemChecked(item))"
          >
            <svg v-if="isItemChecked(item)" viewBox="0 0 14 14" aria-hidden="true">
              <path
                d="M3.2 7.5 5.9 10 10.8 4.4"
                fill="none"
                stroke="currentColor"
                stroke-width="1.9"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </button>

          <span class="related-ico">
            <AppIcon
              v-if="iconKind(item) === 'app'"
              :icon="appIcon ?? null"
              :name="appName"
              :size="18"
            />
            <svg v-else-if="iconKind(item) === 'folder'" aria-hidden="true">
              <use href="#un-folder" />
            </svg>
            <svg v-else-if="iconKind(item) === 'registry'" aria-hidden="true">
              <use href="#un-registry" />
            </svg>
            <svg v-else aria-hidden="true">
              <use href="#un-doc" />
            </svg>
          </span>

          <span class="related-name">{{ item.name }}</span>
          <span v-if="item.kind === 'app_bundle' && item.version" class="related-ver">
            {{ item.version }}
          </span>
          <span class="related-size">{{ formatBytes(item.size_bytes) }}</span>
        </div>
      </div>
    </section>
  </div>
</template>
