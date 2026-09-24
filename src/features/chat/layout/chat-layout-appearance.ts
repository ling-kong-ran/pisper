import type { CSSProperties } from 'react'
import type { ChatLayoutAppearance, ChatLayoutTemplate } from './chat-layout'

type ChatLayoutStyle = CSSProperties & Partial<Record<`--${string}`, string>>

const fontFamilies = {
  system:
    'ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei UI", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif',
  serif: 'ui-serif, "Noto Serif CJK SC", "Songti SC", SimSun, Georgia, serif',
}

const accentColors = {
  blue: ['#1677e8', '#93c5fd'],
  teal: ['#0f766e', '#86e1d8'],
  violet: ['#7557c8', '#c8b9f4'],
} as const

export const CHAT_LAYOUT_ACCENT_CLASS =
  '[--chat-layout-accent-color:var(--chat-layout-accent-light)] dark:[--chat-layout-accent-color:var(--chat-layout-accent-dark)]'

// 两个槽位保持原 key 与节点类型，只移动现有节点，让读屏与 Tab 顺序跟随视觉顺序。
export function orderChatLayoutSlots<T>(
  position: ChatLayoutAppearance['composerPosition'],
  slots: [T, T],
): [T, T] {
  return position === 'top' ? [slots[1], slots[0]] : slots
}

// 所有变量只挂在会话根节点；继承选项不写覆盖值，以继续响应应用的字体与主题偏好。
export function chatLayoutAppearanceStyle(
  appearance: ChatLayoutAppearance,
  accent: ChatLayoutTemplate['accent'],
): ChatLayoutStyle {
  const style: ChatLayoutStyle = {
    '--chat-content-width': `${appearance.contentWidth}px`,
    '--chat-message-gap': appearance.density === 'compact' ? '20px' : '32px',
    '--chat-message-paragraph-gap': appearance.density === 'compact' ? '.55em' : '1em',
    '--chat-transcript-block-padding': appearance.density === 'compact' ? '16px' : '30px',
  }
  if (appearance.fontSize !== null) style['--app-message-font-size'] = `${appearance.fontSize}px`
  if (appearance.fontFamily !== 'inherit') style.fontFamily = fontFamilies[appearance.fontFamily]
  if (appearance.messageStyle === 'plain') {
    style['--user-bubble-text'] = 'var(--text)'
  }
  if (accent !== 'inherit') {
    const [light, dark] = accentColors[accent]
    style['--chat-layout-accent-light'] = light
    style['--chat-layout-accent-dark'] = dark
    style['--brand-blue'] = 'var(--chat-layout-accent-color)'
    style['--brand-blue-hover'] = 'var(--chat-layout-accent-color)'
    style['--brand-blue-strong'] = 'var(--chat-layout-accent-color)'
    style['--brand-blue-soft'] =
      'color-mix(in srgb, var(--chat-layout-accent-color) 12%, transparent)'
    style['--brand-blue-border'] =
      'color-mix(in srgb, var(--chat-layout-accent-color) 30%, transparent)'
    style['--star'] = 'var(--brand-blue)'
    style['--star-hover'] = 'var(--brand-blue-hover)'
    style['--star-strong'] = 'var(--brand-blue-strong)'
    style['--star-soft'] = 'var(--brand-blue-soft)'
    style['--star-border'] = 'var(--brand-blue-border)'
    style['--accent-soft'] = 'var(--brand-blue-soft)'
    style['--accent-strong'] = 'var(--brand-blue-strong)'
    style['--accent-border'] = 'var(--brand-blue-border)'
    style['--focus'] = 'var(--brand-blue)'
    style['--focus-ring'] = 'var(--brand-blue-border)'
  }
  return style
}

// 颜色与标题变化不影响行高，不清空虚拟列表缓存；字体、间距和版式变化需要重新测量。
export function chatLayoutMeasurementKey(appearance: ChatLayoutAppearance): string {
  return [
    appearance.contentWidth,
    appearance.fontSize,
    appearance.fontFamily,
    appearance.density,
    appearance.messageStyle,
  ].join(':')
}
