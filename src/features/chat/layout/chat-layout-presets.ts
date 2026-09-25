import type { ChatCanvasNode } from './chat-canvas'

// Studio 使用主题令牌与普通文档流；右侧上下文继续由外层布局响应窗口宽度。
export function createStudioCanvas(mobile: boolean): ChatCanvasNode {
  return {
    id: 'studio-root',
    kind: 'column',
    css:
      [
        'height: 100%',
        'min-height: 0',
        'box-sizing: border-box',
        `padding: ${mobile ? '6px' : '12px'}`,
        `gap: ${mobile ? '6px' : '10px'}`,
        'background: color-mix(in srgb, var(--muted) 55%, var(--background))',
        'color: var(--foreground)',
        'letter-spacing: .01em',
        '--user-bubble-bg: var(--muted)',
        '--user-bubble-text: var(--foreground)',
        '--chat-message-gap: 28px',
        `--chat-transcript-block-padding: ${mobile ? '16px' : '24px'}`,
        '--chat-message-paragraph-gap: .8em',
      ].join('; ') + ';',
    children: [
      {
        id: 'studio-header',
        kind: 'header',
        css: 'flex-shrink: 0; border-radius: 12px; overflow: hidden;',
      },
      {
        id: 'studio-conversation',
        kind: 'column',
        css:
          [
            'flex: 1 1 0%',
            'min-height: 0',
            'min-width: 0',
            'overflow: hidden',
            'background: var(--card)',
            'border: 1px solid var(--border)',
            `border-radius: ${mobile ? '14px' : '20px'}`,
            'box-shadow: 0 8px 24px color-mix(in srgb, var(--foreground) 5%, transparent)',
          ].join('; ') + ';',
        children: [
          {
            id: 'studio-messages',
            kind: 'messages',
            css: 'flex: 1 1 0%; min-height: 0;',
          },
          {
            id: 'studio-divider',
            kind: 'divider',
            css: `margin-inline: ${mobile ? '12px' : '24px'}; border-color: var(--border);`,
          },
          {
            id: 'studio-composer',
            kind: 'composer',
            css: `flex-shrink: 0; padding-bottom: ${mobile ? '2px' : '6px'};`,
          },
        ],
      },
    ],
  }
}
