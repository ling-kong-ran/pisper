// 会话树纯数据模型：节点类型、树构建/过滤/扁平化与分段（segment）算法。
// 从 SessionTreeDialog 拆出，不依赖 React，便于复用与测试。
import {
  Bot,
  Bookmark,
  FileText,
  GitBranch,
  MessageSquare,
  Settings,
  Tag,
  User,
  Wrench,
} from 'lucide-react'
import type { SessionTreeNode } from '@/features/chat/chat-api'

export type TreeView = 'conversation' | 'labeled' | 'all' | 'marks'
export type DisplayNode = Omit<SessionTreeNode, 'children'> & {
  children: DisplayNode[]
}
export type TreeSegment = {
  id: string
  nodes: DisplayNode[]
  children: TreeSegment[]
  active: boolean
}

export const conversationKinds = new Set(['user', 'assistant', 'summary', 'compaction', 'position'])
export const nonDerivableKinds = new Set(['tool', 'tool-call'])
export const TREE_ROW_HEIGHT = 56
export const TREE_OVERSCAN = 12
export const TREE_VIRTUALIZE_THRESHOLD = 120
export const TREE_TRACK_SCROLL_MARGIN = 26

export function buildDisplayTree(nodes: SessionTreeNode[]): DisplayNode[] {
  const roots: DisplayNode[] = []
  const byId = new Map<string, DisplayNode>()
  for (const node of nodes) {
    const parent = node.parentId ? byId.get(node.parentId) : undefined
    const displayNode: DisplayNode = {
      ...node,
      children: [],
    }
    byId.set(node.id, displayNode)
    if (parent) parent.children.push(displayNode)
    else roots.push(displayNode)
  }
  return roots
}

export function flattenTree(nodes: DisplayNode[]): DisplayNode[] {
  const flattened: DisplayNode[] = []
  const stack = [...nodes].reverse()
  while (stack.length > 0) {
    const node = stack.pop()
    if (!node) continue
    flattened.push(node)
    for (let index = node.children.length - 1; index >= 0; index -= 1) {
      stack.push(node.children[index])
    }
  }
  return flattened
}

export function filterTree(
  nodes: DisplayNode[],
  predicate: (node: DisplayNode) => boolean,
): DisplayNode[] {
  const roots: DisplayNode[] = []
  const visibleAncestor = new Map<string, DisplayNode | null>()
  for (const node of flattenTree(nodes)) {
    const parent = node.parentId ? visibleAncestor.get(node.parentId) || null : null
    if (!predicate(node)) {
      visibleAncestor.set(node.id, parent)
      continue
    }
    const visibleNode = { ...node, children: [] }
    visibleAncestor.set(node.id, visibleNode)
    if (parent) parent.children.push(visibleNode)
    else roots.push(visibleNode)
  }
  return roots
}

export function createSegment(node: DisplayNode): TreeSegment {
  const nodes = [node]
  let cursor = node
  while (cursor.children.length === 1) {
    cursor = cursor.children[0]
    nodes.push(cursor)
  }
  const segment: TreeSegment = {
    id: node.id,
    nodes,
    children: [],
    active: nodes.some((entry) => entry.active),
  }
  const stack = [{ segment, children: cursor.children }]
  while (stack.length > 0) {
    const current = stack.pop()
    if (!current) continue
    for (const child of current.children) {
      const childNodes = [child]
      let childCursor = child
      while (childCursor.children.length === 1) {
        childCursor = childCursor.children[0]
        childNodes.push(childCursor)
      }
      const childSegment: TreeSegment = {
        id: child.id,
        nodes: childNodes,
        children: [],
        active: childNodes.some((entry) => entry.active),
      }
      current.segment.children.push(childSegment)
      stack.push({ segment: childSegment, children: childCursor.children })
    }
  }
  return segment
}

export function nodeIcon(node: SessionTreeNode) {
  if (node.kind === 'user') return User
  if (node.kind === 'assistant') return Bot
  if (node.kind === 'tool' || node.kind === 'tool-call') return Wrench
  if (node.kind === 'settings') return Settings
  if (node.kind === 'label') return Tag
  if (node.kind === 'summary' || node.kind === 'compaction') return FileText
  if (node.kind === 'position') return GitBranch
  if (node.label) return Bookmark
  return MessageSquare
}
