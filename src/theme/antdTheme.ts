import { theme as antdTheme } from 'ant-design-vue'
import type { ThemeConfig } from 'ant-design-vue/es/config-provider/context'

/**
 * 把 Developer Minimalism 规范映射成 ant-design-vue 4 的主题。
 *
 * 规范正文见 docs/ui.md：90% 黑白灰、1px 浅边框、4/6/12 圆角、两级阴影。
 * 这里只放「antd 组件树」的 token；页面自己画的元素走 main.css 里的 CSS 变量，
 * 两套取同一组色值，别在组件里另写颜色。
 */
export const SYSTEM_FONT =
  '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, ' +
  '"PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", sans-serif'

export function buildAntdTheme(dark: boolean): ThemeConfig {
  return {
    algorithm: dark ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm,
    token: {
      colorPrimary: dark ? '#3b82f6' : '#2563eb',
      colorSuccess: dark ? '#34d399' : '#10b981',
      colorError: dark ? '#f87171' : '#ef4444',
      colorWarning: dark ? '#fbbf24' : '#f59e0b',
      colorText: dark ? '#f1f5f9' : '#111827',
      colorTextSecondary: dark ? '#cbd5e1' : '#4b5563',
      colorTextTertiary: dark ? '#94a3b8' : '#6b7280',
      colorTextQuaternary: dark ? '#64748b' : '#9ca3af',
      colorBorder: dark ? '#334155' : '#e5e7eb',
      colorBorderSecondary: dark ? '#334155' : '#e5e7eb',
      colorBgLayout: dark ? '#0f172a' : '#f9fafb',
      colorBgContainer: dark ? '#1e293b' : '#ffffff',
      colorBgElevated: dark ? '#1e293b' : '#ffffff',
      colorFillAlter: dark ? 'rgba(51, 65, 85, 0.5)' : '#f9fafb',
      borderRadius: 6,
      borderRadiusSM: 4,
      borderRadiusLG: 12,
      fontSize: 13,
      fontFamily: SYSTEM_FONT,
      boxShadow: '0 1px 3px rgba(0, 0, 0, 0.02)',
      boxShadowSecondary: '0 4px 6px -1px rgba(0, 0, 0, 0.05)',
      controlHeight: 30,
      lineHeight: 1.6,
    },
  }
}
