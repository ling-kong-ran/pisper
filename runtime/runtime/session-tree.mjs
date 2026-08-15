import { createReadStream } from 'node:fs'
import { createInterface } from 'node:readline'
import { isCompletedTurnBoundaryMessage } from './session-derivation.mjs'

const TREE_NAVIGATION_CUSTOM_TYPE = 'pisper.session-tree-position'
const MAX_NODE_TEXT = 320

function boundedText(value, limit = MAX_NODE_TEXT) {
  const text = String(value || '')
    .replace(/\s+/g, ' ')
    .trim()
  if (text.length <= limit) return text
  return `${text.slice(0, Math.max(0, limit - 1)).trimEnd()}…`
}

function contentText(content) {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content
    .filter((part) => part?.type === 'text' && typeof part.text === 'string')
    .map((part) => part.text)
    .join('\n')
}

function messageProjection(message) {
  const role = String(message?.role || 'message')
  if (role === 'toolResult') {
    return {
      kind: 'tool',
      role,
      text: boundedText(message?.toolName || message?.toolCallId || ''),
      status: message?.isError ? 'error' : 'completed',
    }
  }
  const text = boundedText(contentText(message?.content))
  if (role === 'assistant' && !text && Array.isArray(message?.content)) {
    const tools = message.content
      .filter((part) => part?.type === 'toolCall' && typeof part.name === 'string')
      .map((part) => part.name)
    return { kind: 'assistant', role, text: boundedText(tools.join(', ')), status: '' }
  }
  return {
    kind: role === 'user' ? 'user' : role === 'assistant' ? 'assistant' : 'message',
    role,
    text,
    status: isCompletedTurnBoundaryMessage(message) ? 'completed' : '',
  }
}

function entryProjection(entry) {
  if (entry.type === 'message') return messageProjection(entry.message)
  if (entry.type === 'branch_summary') {
    return { kind: 'summary', role: '', text: boundedText(entry.summary), status: '' }
  }
  if (entry.type === 'compaction') {
    return { kind: 'compaction', role: '', text: boundedText(entry.summary), status: '' }
  }
  if (entry.type === 'custom_message') {
    return {
      kind: entry.display ? 'extension' : 'metadata',
      role: '',
      text: entry.display ? boundedText(contentText(entry.content)) : '',
      status: '',
    }
  }
  if (entry.type === 'model_change') {
    return {
      kind: 'settings',
      role: '',
      text: boundedText(`${entry.provider}/${entry.modelId}`),
      status: '',
    }
  }
  if (entry.type === 'thinking_level_change') {
    return { kind: 'settings', role: '', text: boundedText(entry.thinkingLevel), status: '' }
  }
  if (entry.type === 'session_info') {
    return { kind: 'metadata', role: '', text: boundedText(entry.name), status: '' }
  }
  if (entry.type === 'label') {
    return { kind: 'label', role: '', text: boundedText(entry.label), status: '' }
  }
  if (entry.type === 'custom' && entry.customType === TREE_NAVIGATION_CUSTOM_TYPE) {
    return { kind: 'position', role: '', text: '', status: '' }
  }
  if (entry.type === 'custom') {
    return { kind: 'extension', role: '', text: boundedText(entry.customType), status: '' }
  }
  return { kind: 'metadata', role: '', text: boundedText(entry.type), status: '' }
}

function projectNode(node, activeIds, leafId) {
  const entry = node.entry
  const projected = entryProjection(entry)
  return {
    id: entry.id,
    parentId: entry.parentId || null,
    type: entry.type,
    kind: projected.kind,
    role: projected.role,
    text: projected.text,
    status: projected.status,
    label: boundedText(node.label, 80),
    timestamp: entry.timestamp || '',
    active: activeIds.has(entry.id),
    leaf: entry.id === leafId,
    branchPoint: (node.children || []).length > 1,
  }
}

export function projectSessionTree(manager, { sessionId = '', streaming = false } = {}) {
  const leafId = manager.getLeafId() || null
  const activeIds = new Set(manager.getBranch().map((entry) => entry.id))
  const roots = manager.getTree()
  const nodes = []
  const stack = [...roots].reverse()
  let branchCount = 0
  while (stack.length > 0) {
    const node = stack.pop()
    const children = node.children || []
    nodes.push(projectNode(node, activeIds, leafId))
    branchCount += Math.max(0, children.length - 1)
    for (let index = children.length - 1; index >= 0; index -= 1) stack.push(children[index])
  }
  return {
    sessionId: sessionId || manager.getSessionId(),
    leafId,
    nodeCount: nodes.length,
    branchCount,
    streaming: Boolean(streaming),
    nodes,
  }
}

export function projectSessionTreeLabels(manager, session = {}) {
  const tree = projectSessionTree(manager, { sessionId: session.id || '' })
  const labels = []
  for (const node of tree.nodes) {
    if (!node.label || node.kind !== 'assistant' || node.status !== 'completed') continue
    labels.push({
      sessionId: tree.sessionId,
      sessionName: String(session.name || ''),
      sessionCreated: String(session.created || ''),
      sessionModified: String(session.modified || ''),
      entryId: node.id,
      label: node.label,
      summary: node.text,
      nodeTimestamp: String(node.timestamp || ''),
      active: node.active,
    })
  }
  return labels
}

export function appendTreePosition(manager, targetId) {
  return manager.appendCustomEntry(TREE_NAVIGATION_CUSTOM_TYPE, { targetId })
}

export async function scanSessionTreeLabels(path, session = {}) {
  if (!path) return []
  const labelEntries = []
  const parentById = new Map()
  const assistantIds = new Set()
  let header = null
  let name = ''
  let lastId = ''
  const lines = createInterface({
    input: createReadStream(path, { encoding: 'utf8' }),
    crlfDelay: Infinity,
  })
  const field = (line, key) => {
    const needle = `"${key}":"`
    const start = line.indexOf(needle)
    if (start < 0) return null
    const from = start + needle.length
    const end = line.indexOf('"', from)
    return end < 0 ? null : line.slice(from, end)
  }
  for await (const line of lines) {
    if (line.startsWith('{"type":"session"')) {
      if (!header) {
        try {
          header = JSON.parse(line)
        } catch {}
      }
      continue
    }
    if (line.startsWith('{"type":"session_info"')) {
      try {
        const entry = JSON.parse(line)
        if (entry.name) name = entry.name
      } catch {}
      continue
    }
    if (line.startsWith('{"type":"label"')) {
      try {
        const entry = JSON.parse(line)
        if (entry.label && entry.targetId) {
          labelEntries.push({ entryId: entry.targetId, label: entry.label, timestamp: entry.timestamp })
        }
      } catch {}
      continue
    }
    const id = field(line, 'id')
    if (!id) continue
    lastId = id
    parentById.set(id, field(line, 'parentId'))
    if (line.startsWith('{"type":"message"') && line.includes('"role":"assistant"')) {
      assistantIds.add(id)
    }
  }
  const completed = labelEntries.filter((labelEntry) => assistantIds.has(labelEntry.entryId))
  if (!completed.length) return []
  const activeIds = new Set()
  for (let cursor = lastId; cursor; cursor = parentById.get(cursor) || '') {
    if (activeIds.has(cursor)) break
    activeIds.add(cursor)
  }
  const iso = (value) => (value instanceof Date ? value.toISOString() : String(value || ''))
  return completed.map((labelEntry) => ({
    sessionId: session.id || header?.id || '',
    sessionName: String(session.name || name || ''),
    sessionCreated: iso(session.created || header?.timestamp),
    sessionModified: iso(session.modified),
    entryId: labelEntry.entryId,
    label: labelEntry.label,
    summary: '',
    nodeTimestamp: String(labelEntry.timestamp || ''),
    active: activeIds.has(labelEntry.entryId),
  }))
}

export { TREE_NAVIGATION_CUSTOM_TYPE }
