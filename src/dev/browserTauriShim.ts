/**
 * 浏览器里预览界面用的 Tauri 兜底（只在 DEV + 非 Tauri 环境生效）。
 *
 * `pnpm dev` 起的是纯 Vite 页面，没有 Tauri 注入的 `__TAURI_INTERNALS__`，
 * `getCurrentWindow()` 一调就抛 `Cannot read properties of undefined (reading 'metadata')`，
 * 整个 `#app` 挂不上去，也就没法在浏览器里看样式。
 *
 * 这里给出一份最小可用的假后端：窗口元数据、事件注册、以及首页/设置/LANLink
 * 会用到的几个只读命令。真实 Tauri 窗口里 `__TAURI_INTERNALS__` 已存在，这个函数直接返回，
 * 不会影响桌面应用；打包产物里也会被 `import.meta.env.DEV` 挡掉。
 */
declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown
  }
}

const PEER_ID = 'aa:bb:cc:dd:ee:ff'
const DAY = 86_400_000

function windowLabel(): string {
  const path = typeof location === 'undefined' ? '' : location.pathname
  if (path.endsWith('quick.html')) return 'quick'
  if (path.endsWith('clipboard.html')) return 'clipboard'
  if (path.endsWith('uninstall.html')) return 'uninstall'
  return 'main'
}

function fixtures() {
  const now = Date.now()
  return {
    config: {
      groups: [
        {
          id: 'group-preview',
          name: '默认分组',
          is_default: true,
          created_at: new Date(now - DAY * 30).toISOString(),
          note: '',
          attachments: [],
          projects: [
            {
              id: 'project-preview',
              name: 'ws-workbench',
              source_dir: '/Users/you/code/ws-workbench',
              output_dir: '/Users/you/tmp/out',
              created_at: new Date(now - DAY * 20).toISOString(),
              updated_at: new Date(now - DAY).toISOString(),
              note: '',
              attachments: [],
            },
          ],
        },
      ],
      projects: [],
      tree_order: ['group-preview'],
      default_exclude: ['.*', 'docs/**', '*.md', '*.scss', '*.less', '*.map', '*.test.js', '*.spec.js', '__tests__', 'node_modules', '*.zip'],
      default_exclude_version: 4,
      vpn_profiles: [
        {
          id: 'vpn-preview',
          name: '公司内网',
          server: 'vpn.example.com:443',
          username: 'you',
          password: '',
          password_encrypted: true,
          protocol: 'anyconnect',
          connector: 'auto',
          proxy_mode: 'rule',
          rules: [
            {
              id: 'rule-1',
              kind: 'DOMAIN-SUFFIX',
              value: 'example.com',
              policy: 'VPN',
              remark: '内网域名',
              enabled: true,
            },
            {
              id: 'rule-2',
              kind: 'IP-CIDR',
              value: '10.0.0.0/8',
              policy: 'DIRECT',
              remark: '',
              enabled: true,
            },
          ],
          rules_default_policy: 'DIRECT',
          extra_args: [],
          connector_path: '',
          created_at: new Date(now - DAY * 10).toISOString(),
          updated_at: new Date(now - DAY).toISOString(),
        },
      ],
      vpn_rule_sets: [],
      vpn_active_profile_id: 'vpn-preview',
      default_upload_dir: null,
      developer_mode: false,
    },
    lanSnapshot: {
      enabled: true,
      running: true,
      alias: 'fengwenshandeMacBook-Pro.local',
      fingerprint: 'EA:80:73:3B:CB:AE',
      port: 53317,
      download_dir: '/Users/you/Downloads',
      download_dir_custom: false,
      local_addresses: ['192.168.1.110'],
      peers: [
        {
          fingerprint: PEER_ID,
          alias: '预览设备',
          ip: '192.168.1.120',
          port: 53317,
          device_type: 'windows',
          addresses: ['192.168.1.120'],
          last_seen_ms: now - 60_000,
        },
      ],
      pending_offers: [],
      require_accept: false,
      peer_remarks: {},
      last_error: null,
      warning: null,
    },
    lanConversations: [
      {
        id: PEER_ID,
        alias: '预览设备',
        online: false,
        last_activity_ms: now - DAY,
        last_preview: '这是一条预览消息',
        unread: 0,
        created_at_ms: now - DAY,
      },
    ],
    lanMessages: [
      {
        id: 'preview-1',
        peer_id: PEER_ID,
        direction: 'out',
        kind: 'text',
        text: '这是一条预览消息',
        file_name: null,
        file_size: null,
        file_path: null,
        status: 'sent',
        error: null,
        transfer_id: null,
        file_index: null,
        file_total: null,
        created_at_ms: now - DAY + 1000,
      },
    ],
  }
}

export function installBrowserTauriShim(): void {
  if (typeof window === 'undefined') return
  if (window.__TAURI_INTERNALS__) return

  const label = windowLabel()
  const data = fixtures()
  let nextId = 1

  const handlers: Record<string, () => unknown> = {
    get_config: () => data.config,
    app_name: () => 'ws-workbench',
    dev_build: () => false,
    is_prerelease_build: () => false,
    get_platform_info: () => ({
      os: 'macos',
      label: 'macOS',
      arch: '',
      target: '',
      is_windows: false,
      is_macos: true,
      is_linux: false,
      exe_suffix: '',
      vpn_needs_admin: true,
    }),
    app_ready: () => null,
    'plugin:app|version': () => '1.0.0',
    'plugin:event|listen': () => nextId++,
    'plugin:event|unlisten': () => null,
    'plugin:event|emit': () => null,
    'plugin:window|is_maximized': () => false,
    'plugin:window|scale_factor': () => 2,
    'plugin:window|theme': () => null,
    vault_encrypt: () => 'v1.preview',
    vault_decrypt: () => '',
    lan_share_snapshot: () => data.lanSnapshot,
    lan_share_unread_total: () => 0,
    lan_share_conversations: () => data.lanConversations,
    lan_share_messages: () => data.lanMessages,
    lan_share_mark_read: () => null,
    clipboard_limits: () => ({ shortcut: 'CommandOrControl+Shift+V', max_text_chars: 20000 }),
    clipboard_page: () => ({ items: [], has_more: false }),
    clipboard_search: () => [],
    clipboard_stats: () => ({ items: 0, images: 0, bytes: 0 }),
    list_open_with_apps: () => [],
    refresh_open_with_apps: () => [],
    vpn_detect_connectors: () => [
      {
        kind: 'openconnect',
        name: 'openconnect（内置）',
        path: '/Applications/ws-workbench.app/Contents/Resources/openconnect',
        version: '9.12',
        available: true,
        source: 'bundled',
        note: '',
      },
    ],
  }

  const internals = {
    metadata: {
      currentWindow: { label },
      currentWebview: { label },
    },
    invoke: async (cmd: string) => {
      const handler = handlers[cmd]
      return handler ? handler() : null
    },
    transformCallback: (callback: (payload: unknown) => void) => {
      const id = nextId++
      ;(window as unknown as Record<string, unknown>)[`_${id}`] = callback
      return id
    },
    unregisterCallback: (id: number) => {
      delete (window as unknown as Record<string, unknown>)[`_${id}`]
    },
    convertFileSrc: (path: string) => path,
  }

  window.__TAURI_INTERNALS__ = internals
  const withEventPlugin = window as unknown as {
    __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: (event: string, eventId: number) => void }
  }
  withEventPlugin.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
}
