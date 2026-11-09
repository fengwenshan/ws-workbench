import { platform } from '@/platform'

/**
 * 全局快捷键的录制与展示。
 *
 * 存储用加速键字符串（`CommandOrControl+Shift+V`，与 Rust 侧 global-hotkey 的解析一致）：
 * - 前端只负责把按键翻译成字符串、把字符串按平台渲染成 ⌘⇧V / Ctrl+Shift+V；
 * - 真正是否可用（格式、是否被别的应用占用）由后端注册时判断。
 */

/** 只按修饰键时不录制 */
const MODIFIER_CODES = new Set([
  'ControlLeft',
  'ControlRight',
  'ShiftLeft',
  'ShiftRight',
  'AltLeft',
  'AltRight',
  'MetaLeft',
  'MetaRight',
  'CapsLock',
  'NumLock',
  'ScrollLock',
  'Fn',
  'FnLock',
])

/** 有专门符号的键；其余 KeyA / Digit1 / F5 之类按 name 规则推出来 */
const NAMED_KEYS: Record<string, string> = {
  Space: 'Space',
  Tab: 'Tab',
  Enter: 'Enter',
  Backquote: '`',
  Minus: '-',
  Equal: '=',
  BracketLeft: '[',
  BracketRight: ']',
  Backslash: '\\',
  Semicolon: ';',
  Quote: "'",
  Comma: ',',
  Period: '.',
  Slash: '/',
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
  Home: 'Home',
  End: 'End',
  PageUp: 'PageUp',
  PageDown: 'PageDown',
  Insert: 'Insert',
  Delete: 'Delete',
  Backspace: 'Backspace',
  PrintScreen: 'PrintScreen',
}

/** 展示用的键名：加速键里的写法 → 界面上看的字符 */
const DISPLAY_KEYS: Record<string, string> = {
  BACKQUOTE: '`',
  MINUS: '-',
  EQUAL: '=',
  BRACKETLEFT: '[',
  BRACKETRIGHT: ']',
  BACKSLASH: '\\',
  SEMICOLON: ';',
  QUOTE: "'",
  COMMA: ',',
  PERIOD: '.',
  SLASH: '/',
  UP: '↑',
  DOWN: '↓',
  LEFT: '←',
  RIGHT: '→',
  SPACE: 'Space',
  TAB: 'Tab',
  ENTER: 'Enter',
}

/** KeyboardEvent.code → 加速键里表示主键的写法（大写，global-hotkey 能解析） */
function keyName(code: string): string | null {
  if (code in NAMED_KEYS) return NAMED_KEYS[code]
  const letter = /^Key([A-Z])$/.exec(code)
  if (letter) return letter[1]
  const digit = /^Digit(\d)$/.exec(code)
  if (digit) return digit[1]
  if (/^F\d{1,2}$/.test(code)) return code
  return null
}

/**
 * 把一次按键翻译成加速键字符串；只按了修饰键、或没有真正的修饰键时返回 null。
 *
 * macOS 上 Command 记成 `CommandOrControl`，Windows 上 Ctrl 记成 `Control`，
 * 这样默认组合两边都能解析成各自习惯的键。
 */
export function shortcutFromEvent(event: KeyboardEvent): string | null {
  if (MODIFIER_CODES.has(event.code)) return null
  const key = keyName(event.code)
  if (!key) return null

  const modifiers: string[] = []
  if (platform.is_macos) {
    if (event.metaKey) modifiers.push('CommandOrControl')
    if (event.ctrlKey) modifiers.push('Control')
  } else {
    if (event.ctrlKey) modifiers.push('Control')
    if (event.metaKey) modifiers.push('Super')
  }
  if (event.altKey) modifiers.push('Alt')
  if (event.shiftKey) modifiers.push('Shift')

  // 全局快捷键至少要有一个真正的修饰键，Shift 单独按不算
  if (!modifiers.some((modifier) => modifier !== 'Shift')) return null
  return [...modifiers, key].join('+')
}

/** 加速键字符串 → 当前平台上习惯的显示写法（mac ⌘⇧V / Windows Ctrl+Shift+V） */
export function formatShortcut(accelerator: string): string {
  const tokens = accelerator
    .split('+')
    .map((token) => token.trim())
    .filter(Boolean)
  if (tokens.length === 0) return ''

  const parts = tokens.map((token) => displayToken(token, platform.is_macos))
  return platform.is_macos ? parts.join('') : parts.join('+')
}

function displayToken(token: string, isMac: boolean): string {
  const upper = token.toUpperCase()
  const macSymbols: Record<string, string> = {
    COMMANDORCONTROL: '⌘',
    COMMANDORCTRL: '⌘',
    CMDORCTRL: '⌘',
    CMDORCONTROL: '⌘',
    COMMAND: '⌘',
    CMD: '⌘',
    SUPER: '⌘',
    CONTROL: '⌃',
    CTRL: '⌃',
    ALT: '⌥',
    OPTION: '⌥',
    SHIFT: '⇧',
  }
  if (isMac) {
    if (upper in macSymbols) return macSymbols[upper]
  } else {
    switch (upper) {
      case 'COMMANDORCONTROL':
      case 'COMMANDORCTRL':
      case 'CMDORCTRL':
      case 'CMDORCONTROL':
      case 'COMMAND':
      case 'CMD':
      case 'CONTROL':
      case 'CTRL':
        return 'Ctrl'
      case 'SUPER':
        return 'Win'
      case 'ALT':
      case 'OPTION':
        return 'Alt'
      case 'SHIFT':
        return 'Shift'
    }
  }
  if (upper in DISPLAY_KEYS) return DISPLAY_KEYS[upper]
  return upper.length === 1 ? upper : token
}
