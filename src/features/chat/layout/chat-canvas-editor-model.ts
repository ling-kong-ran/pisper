import {
  addCanvasNode,
  findCanvasNode,
  isCanvasContainerKind,
  updateCanvasNode,
  type ChatCanvasKind,
  type ChatCanvasNode,
} from './chat-canvas'
import { parseCanvasCss, splitCanvasCssDeclarations } from './chat-canvas-style'

type Translate = (key: string) => string

export function canvasKindLabels(t: Translate): Record<ChatCanvasKind, string> {
  return {
    row: t('chat-layout:canvas.row'),
    column: t('chat-layout:canvas.column'),
    grid: t('chat-layout:canvas.grid'),
    text: t('chat-layout:canvas.text'),
    divider: t('chat-layout:canvas.divider'),
    spacer: t('chat-layout:canvas.spacer'),
    header: t('chat-layout:canvas.header'),
    messages: t('chat-layout:canvas.messages'),
    composer: t('chat-layout:canvas.composer'),
    model: t('chat-layout:canvas.model'),
    tools: t('chat-layout:canvas.tools'),
    usage: t('chat-layout:canvas.usage'),
    workspace: t('chat-layout:canvas.workspace'),
    context: t('chat-layout:canvas.context'),
  }
}

export function canvasParent(root: ChatCanvasNode, id: string): ChatCanvasNode | undefined {
  if (root.children?.some((node) => node.id === id)) return root
  for (const child of root.children ?? []) {
    const parent = canvasParent(child, id)
    if (parent) return parent
  }
}

export function canvasNodes(root: ChatCanvasNode): ChatCanvasNode[] {
  return [root, ...(root.children ?? []).flatMap(canvasNodes)]
}

export function canCopyCanvasNode(node: ChatCanvasNode): boolean {
  return canvasNodes(node).every(
    (entry) =>
      isCanvasContainerKind(entry.kind) || ['text', 'divider', 'spacer'].includes(entry.kind),
  )
}

export function canRemoveCanvasNode(root: ChatCanvasNode, node: ChatCanvasNode): boolean {
  return (
    node.id !== root.id &&
    !canvasNodes(node).some((entry) => entry.kind === 'messages' || entry.kind === 'composer')
  )
}

export function copyCanvasNode(root: ChatCanvasNode, id: string): ChatCanvasNode {
  const node = findCanvasNode(root, id)
  const parent = canvasParent(root, id)
  if (!node || !parent || !canCopyCanvasNode(node)) return root
  const append = (
    tree: ChatCanvasNode,
    source: ChatCanvasNode,
    parentId: string,
  ): ChatCanvasNode => {
    let next = addCanvasNode(tree, parentId, source.kind)
    const copy = findCanvasNode(next, parentId)?.children?.at(-1)
    if (!copy) return tree
    next = updateCanvasNode(next, copy.id, {
      css: source.css,
      ...(source.kind === 'text' ? { text: source.text ?? '' } : {}),
    })
    for (const child of source.children ?? []) next = append(next, child, copy.id)
    return next
  }
  return append(root, node, parent.id)
}

export const CANVAS_NODE_DRAG = 'application/x-pisper-canvas-node'
export const CANVAS_KIND_DRAG = 'application/x-pisper-canvas-kind'

export function replaceCanvasCssDeclaration(css: string, property: string, value: string): string {
  const declarations = splitCanvasCssDeclarations(css).filter(
    (declaration) =>
      declaration.slice(0, declaration.indexOf(':')).trim().toLowerCase() !== property,
  )
  const next = [...declarations, `${property}: ${value}`].join(';\n') + ';'
  parseCanvasCss(next)
  return next
}
