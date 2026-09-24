import type { CSSProperties } from 'react'
import {
  CHAT_CANVAS_SLOT_KINDS,
  type ChatCanvasKind,
  type ChatCanvasNode,
  type ChatCanvasSlotKind,
} from './chat-canvas'
import { parseCanvasCss } from './chat-canvas-style'

export { CHAT_CANVAS_SLOT_KINDS }
export type { ChatCanvasSlotKind }

export function isFloatingCanvasIsland(node: ChatCanvasNode): boolean {
  return node.kind === 'custom-ui' && node.componentId === 'pisper-island'
}

export function collectFloatingCanvasIslands(root: ChatCanvasNode): ChatCanvasNode[] {
  if (isFloatingCanvasIsland(root)) return [root]
  return (root.children ?? []).flatMap(collectFloatingCanvasIslands)
}

export function isChatCanvasSlotKind(kind: ChatCanvasKind): kind is ChatCanvasSlotKind {
  return CHAT_CANVAS_SLOT_KINDS.some((slot) => slot === kind)
}

export function collectChatCanvasSlots(root: ChatCanvasNode): ChatCanvasSlotKind[] {
  const kinds: ChatCanvasSlotKind[] = []
  const visit = (node: ChatCanvasNode) => {
    if (isChatCanvasSlotKind(node.kind)) kinds.push(node.kind)
    node.children?.forEach(visit)
  }
  visit(root)
  return kinds
}

function hasGrowingContent(node: ChatCanvasNode): boolean {
  return (
    node.kind === 'messages' ||
    node.kind === 'context' ||
    Boolean(node.children?.some(hasGrowingContent))
  )
}

// 预览与真实画布共用默认盒模型；自定义声明只覆盖本节点，不改全局主题或其他会话。
export function chatCanvasNodeStyle(node: ChatCanvasNode, isRoot = false): CSSProperties {
  const style: CSSProperties & Partial<Record<`--${string}`, string>> = {
    minWidth: 0,
    minHeight: 0,
    flex: hasGrowingContent(node) ? '1 1 0%' : '0 0 auto',
  }
  if (node.kind === 'row' || node.kind === 'column') {
    style.display = 'flex'
    style.flexDirection = node.kind
  } else if (node.kind === 'grid') {
    style.display = 'grid'
    style.gridTemplateColumns = 'repeat(2, minmax(0, 1fr))'
  } else if (node.kind === 'spacer') {
    style.flex = '1 1 0%'
    style.minHeight = 12
  } else if (node.kind === 'divider') {
    style.borderTop = '1px solid var(--stroke-soft)'
    style.margin = 0
  } else if (isChatCanvasSlotKind(node.kind) || node.kind === 'custom-ui') {
    style.display = 'flex'
    style.flexDirection = 'column'
    if (node.kind === 'messages' || node.kind === 'context') style.overflow = 'hidden'
  }
  if (isRoot) {
    style.width = '100%'
    style.height = '100%'
  }
  const custom = parseCanvasCss(node.css)
  // 消息与输入原本有独立字号变量，使用 1em 继承节点计算值，避免 em/% 再次放大。
  if (custom.fontSize || custom.font) style['--app-message-font-size'] = '1em'
  return { ...style, ...custom }
}

// 容器只按功能身份创建一次。移动画布节点不改变 portal 的挂载容器，组件状态可以延续。
export function createChatCanvasHosts<T>(createHost: (kind: ChatCanvasSlotKind) => T) {
  const hosts = new Map<ChatCanvasSlotKind, { value: T }>()
  return (kind: ChatCanvasSlotKind): T => {
    const existing = hosts.get(kind)
    if (existing) return existing.value
    const host = createHost(kind)
    hosts.set(kind, { value: host })
    return host
  }
}
