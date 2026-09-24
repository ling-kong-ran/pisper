import { parseCanvasCss } from './chat-canvas-style'
import { ChatLayoutValidationError } from './chat-layout-error'

export const CANVAS_KINDS = [
  'row',
  'column',
  'grid',
  'text',
  'divider',
  'spacer',
  'header',
  'messages',
  'composer',
  'model',
  'tools',
  'usage',
  'workspace',
  'context',
] as const
export type ChatCanvasKind = (typeof CANVAS_KINDS)[number]
export type ChatCanvasNode = {
  id: string
  kind: ChatCanvasKind
  css: string
  text?: string
  children?: ChatCanvasNode[]
}

export const CANVAS_CONTAINER_KINDS = ['row', 'column', 'grid'] as const
export const CANVAS_MAX_NODES = 64
export const CANVAS_MAX_DEPTH = 8
export const CANVAS_TEXT_MAX_LENGTH = 1000
export const CHAT_CANVAS_SLOT_KINDS = [
  'header',
  'messages',
  'composer',
  'model',
  'tools',
  'usage',
  'workspace',
  'context',
] as const
export type ChatCanvasSlotKind = (typeof CHAT_CANVAS_SLOT_KINDS)[number]
const functionalKinds = new Set<ChatCanvasKind>(CHAT_CANVAS_SLOT_KINDS)

export function isCanvasContainerKind(kind: ChatCanvasKind): boolean {
  return kind === 'row' || kind === 'column' || kind === 'grid'
}
export const isCanvasContainer = isCanvasContainerKind

function invalid(path = 'canvas'): never {
  throw new ChatLayoutValidationError('invalid_canvas', path)
}

function record(input: unknown): Record<string, unknown> {
  if (!input || typeof input !== 'object' || Array.isArray(input)) invalid()
  const prototype: unknown = Object.getPrototypeOf(input)
  if (prototype !== Object.prototype && prototype !== null) invalid()
  const value: Record<string, unknown> = {}
  for (const key of Reflect.ownKeys(input)) {
    if (typeof key !== 'string' || !['id', 'kind', 'css', 'text', 'children'].includes(key))
      invalid()
    const descriptor = Object.getOwnPropertyDescriptor(input, key)
    if (!descriptor || !('value' in descriptor)) invalid()
    value[key] = descriptor.value
  }
  return value
}

export function parseChatCanvas(input: unknown): ChatCanvasNode {
  const ids = new Set<string>()
  const functional = new Set<ChatCanvasKind>()
  const visit = (raw: unknown, depth: number): ChatCanvasNode => {
    if (depth > CANVAS_MAX_DEPTH || ids.size >= CANVAS_MAX_NODES) invalid()
    const value = record(raw)
    if (
      typeof value.id !== 'string' ||
      !/^[a-zA-Z][a-zA-Z0-9_-]{0,63}$/.test(value.id) ||
      ids.has(value.id)
    )
      invalid('canvas.id')
    ids.add(value.id)
    const kind = CANVAS_KINDS.find((candidate) => candidate === value.kind)
    if (!kind) invalid('canvas.kind')
    if (functionalKinds.has(kind)) {
      if (functional.has(kind)) invalid('canvas.kind')
      functional.add(kind)
    }
    if (typeof value.css !== 'string') invalid('canvas.css')
    parseCanvasCss(value.css)
    const node: ChatCanvasNode = { id: value.id, kind, css: value.css }
    if (kind === 'text') {
      if (
        value.text !== undefined &&
        (typeof value.text !== 'string' || value.text.length > CANVAS_TEXT_MAX_LENGTH)
      )
        invalid('canvas.text')
      node.text = value.text ?? ''
    } else if ('text' in value) invalid('canvas.text')
    if (isCanvasContainerKind(kind)) {
      if (value.children !== undefined && !Array.isArray(value.children)) invalid('canvas.children')
      node.children = Array.isArray(value.children)
        ? value.children.map((child) => visit(child, depth + 1))
        : []
    } else if ('children' in value) invalid('canvas.children')
    return node
  }
  const root = visit(input, 1)
  if (
    !isCanvasContainerKind(root.kind) ||
    !functional.has('messages') ||
    !functional.has('composer')
  )
    invalid()
  return root
}

export function createDefaultCanvas(appearance: {
  composerPosition: 'top' | 'bottom'
}): ChatCanvasNode {
  const header: ChatCanvasNode = { id: 'canvas-header', kind: 'header', css: 'flex-shrink: 0;' }
  const messages: ChatCanvasNode = {
    id: 'canvas-messages',
    kind: 'messages',
    css: 'flex: 1; min-height: 0;',
  }
  const composer: ChatCanvasNode = {
    id: 'canvas-composer',
    kind: 'composer',
    css: 'flex-shrink: 0;',
  }
  return {
    id: 'canvas-root',
    kind: 'column',
    css: 'height: 100%; min-height: 0;',
    children:
      appearance.composerPosition === 'top'
        ? [header, composer, messages]
        : [header, messages, composer],
  }
}

export function findCanvasNode(root: ChatCanvasNode, id: string): ChatCanvasNode | undefined {
  if (root.id === id) return root
  for (const child of root.children ?? []) {
    const found = findCanvasNode(child, id)
    if (found) return found
  }
  return undefined
}

export function canvasHasKind(root: ChatCanvasNode, kind: ChatCanvasKind): boolean {
  return root.kind === kind || (root.children ?? []).some((child) => canvasHasKind(child, kind))
}

let nextNodeId = 0
function newId(root: ChatCanvasNode, kind: ChatCanvasKind): string {
  let id: string
  do {
    nextNodeId += 1
    id = `canvas-${kind}-${nextNodeId.toString(36)}`
  } while (findCanvasNode(root, id))
  return id
}

export function addCanvasNode(
  root: ChatCanvasNode,
  parentId: string,
  kind: ChatCanvasKind,
): ChatCanvasNode {
  const next = parseChatCanvas(root)
  const parent = findCanvasNode(next, parentId)
  if (!parent || !isCanvasContainerKind(parent.kind)) invalid()
  const node: ChatCanvasNode = { id: newId(next, kind), kind, css: '' }
  if (kind === 'text') node.text = ''
  if (kind === 'spacer') node.css = 'min-height: 16px;'
  if (isCanvasContainerKind(kind)) node.children = []
  parent.children = [...(parent.children ?? []), node]
  return parseChatCanvas(next)
}

function parentOf(root: ChatCanvasNode, id: string): ChatCanvasNode | undefined {
  for (const child of root.children ?? []) {
    if (child.id === id) return root
    const found = parentOf(child, id)
    if (found) return found
  }
  return undefined
}

export function moveCanvasNode(
  root: ChatCanvasNode,
  nodeId: string,
  parentId: string,
  index: number,
): ChatCanvasNode {
  const next = parseChatCanvas(root)
  const node = findCanvasNode(next, nodeId)
  const parent = parentOf(next, nodeId)
  const destination = findCanvasNode(next, parentId)
  if (
    !node ||
    !parent ||
    !destination ||
    !isCanvasContainerKind(destination.kind) ||
    findCanvasNode(node, parentId)
  )
    invalid()
  if (!Number.isInteger(index) || index < 0 || index > (destination.children?.length ?? 0))
    invalid()
  parent.children = parent.children?.filter((child) => child.id !== nodeId)
  const children = destination.children ?? []
  // index 指向移除源节点后的目标序列；传入原长度可作为同父级移动到末尾的快捷写法。
  children.splice(Math.min(index, children.length), 0, node)
  destination.children = children
  return parseChatCanvas(next)
}

export function removeCanvasNode(root: ChatCanvasNode, id: string): ChatCanvasNode {
  const next = parseChatCanvas(root)
  const parent = parentOf(next, id)
  if (!parent) invalid()
  parent.children = parent.children?.filter((child) => child.id !== id)
  return parseChatCanvas(next)
}

export function updateCanvasNode(
  root: ChatCanvasNode,
  id: string,
  patch: { css?: string; text?: string },
): ChatCanvasNode {
  const next = parseChatCanvas(root)
  const node = findCanvasNode(next, id)
  if (!node || !patch || typeof patch !== 'object' || Array.isArray(patch)) invalid()
  for (const key of Reflect.ownKeys(patch)) {
    if (key !== 'css' && key !== 'text') invalid()
    const descriptor = Object.getOwnPropertyDescriptor(patch, key)
    if (!descriptor || !('value' in descriptor) || typeof descriptor.value !== 'string') invalid()
    node[key] = descriptor.value
  }
  return parseChatCanvas(next)
}
