// 会话树投影：把 Pi 的会话分支树（JSONL 文件）转成前端可渲染的树结构，
// 并提供标签（label）条目的增量扫描——标签数量可能很多，全量重扫成本高。
import { createReadStream } from 'node:fs'
import { open, stat } from 'node:fs/promises'
import { createInterface } from 'node:readline'
import { isCompletedTurnBoundaryMessage } from './session-derivation.mjs'

// 树导航用的自定义条目类型：记录“当前查看位置”以便回到上次浏览节点。
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
  if (role === 'assistant' && Array.isArray(message?.content)) {
    const tools = message.content
      .filter((part) => part?.type === 'toolCall' && typeof part.name === 'string')
      .map((part) => part.name)
    if (tools.length > 0) {
      return {
        kind: 'tool-call',
        role,
        text: boundedText([text, tools.join(', ')].filter(Boolean).join(' · ')),
        status: '',
      }
    }
  }
  return {
    kind: role === 'user' ? 'user' : role === 'assistant' ? 'assistant' : 'message',
    role,
    text,
    status: isCompletedTurnBoundaryMessage(message) ? 'completed' : '',
  }
}

// 把单条会话记录投影为前端节点：工具结果/工具调用/分支摘要/压缩/标签等类型各取所需字段。
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

function isTreePosition(node) {
  return node?.entry?.type === 'custom' && node.entry.customType === TREE_NAVIGATION_CUSTOM_TYPE
}

function visibleChildren(node, leafId, positionContext) {
  const children = node?.children || []
  const targetId = node.entry?.id
  const pending = children.filter((child) =>
    isPendingTreePosition(child, targetId, positionContext),
  )
  const keepId =
    pending.length > 1
      ? pending.find((child) => child.entry.id === leafId)?.entry.id || pending[0].entry.id
      : pending[0]?.entry.id
  return children.filter((child) => !isTreePosition(child) || child.entry.id === keepId)
}

function projectNode(node, activeIds, leafId, children = node.children || []) {
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
    branchPoint: children.length > 1,
  }
}

// 深度优先（先根后子、从右往左压栈以保证左子节点先出）投影整棵树，
// 附带活动链、叶节点、分支点等导航信息。
export function projectSessionTree(manager, { sessionId = '', streaming = false } = {}) {
  const leafId = manager.getLeafId() || null
  const activeIds = new Set(manager.getBranch().map((entry) => entry.id))
  const roots = manager.getTree()
  const positionContext = getPositionContext(manager)
  const nodes = []
  const stack = [...roots].reverse()
  let branchCount = 0
  while (stack.length > 0) {
    const node = stack.pop()
    const children = visibleChildren(node, leafId, positionContext)
    nodes.push(projectNode(node, activeIds, leafId, children))
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

// 汇总树中已完成的助手消息标签，供会话标签搜索使用。
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

function hasUserMessageDescendant(node) {
  const stack = [...(node?.children || [])]
  while (stack.length > 0) {
    const current = stack.pop()
    if (current?.entry?.type === 'message' && current.entry.message?.role === 'user') return true
    stack.push(...(current?.children || []))
  }
  return false
}

function isEntryDescendantOf(entryId, ancestorId, entriesById) {
  const visited = new Set()
  let current = entriesById.get(entryId)
  while (current?.parentId && !visited.has(current.id)) {
    if (current.parentId === ancestorId) return true
    visited.add(current.id)
    current = entriesById.get(current.parentId)
  }
  return false
}

function hasUserMessageAfterPosition(positionId, targetId, positionContext) {
  const { entries, entriesById } = positionContext
  const positionIndex = entries.findIndex((entry) => entry?.id === positionId)
  if (positionIndex < 0) return false
  return entries
    .slice(positionIndex + 1)
    .some(
      (entry) =>
        entry?.type === 'message' &&
        entry.message?.role === 'user' &&
        isEntryDescendantOf(entry.id, targetId, entriesById),
    )
}

function isPendingTreePosition(node, targetId, positionContext = null) {
  const entry = node?.entry
  return (
    entry?.type === 'custom' &&
    entry.customType === TREE_NAVIGATION_CUSTOM_TYPE &&
    entry.parentId === targetId &&
    entry.data?.targetId === targetId &&
    !hasUserMessageDescendant(node) &&
    !hasUserMessageAfterPosition(
      entry.id,
      targetId,
      positionContext || { entries: [], entriesById: new Map() },
    )
  )
}

function getPositionContext(manager) {
  const entries = manager?.getEntries?.() || []
  return { entries, entriesById: new Map(entries.map((entry) => [entry.id, entry])) }
}

// 查找目标节点下尚未开始新消息的分支位置；重复导航应复用它，避免制造空分支。
export function findPendingTreePosition(manager, targetId) {
  const normalizedTargetId = String(targetId || '').trim()
  if (!normalizedTargetId || !manager?.getTree) return ''
  const leafId = manager.getLeafId?.() || ''
  const positionContext = getPositionContext(manager)
  const matches = []
  const stack = [...(manager.getTree() || [])].reverse()
  while (stack.length > 0) {
    const node = stack.pop()
    if (node?.entry?.id === normalizedTargetId) {
      for (const child of node.children || []) {
        if (isPendingTreePosition(child, normalizedTargetId, positionContext))
          matches.push(child.entry.id)
      }
      break
    }
    for (let index = (node?.children || []).length - 1; index >= 0; index -= 1)
      stack.push(node.children[index])
  }
  return matches.find((id) => id === leafId) || matches[0] || ''
}

// 追加一个“树导航位置”自定义条目，之后可据此恢复上次的浏览位置。
export function appendTreePosition(manager, targetId) {
  const normalizedTargetId = String(targetId || '').trim()
  const existing = findPendingTreePosition(manager, normalizedTargetId)
  return (
    existing ||
    manager.appendCustomEntry(TREE_NAVIGATION_CUSTOM_TYPE, { targetId: normalizedTargetId })
  )
}

// 判断 offset 处是否恰好是行首（前一字节是换行），用于增量读取的边界对齐。
export async function isLineBoundary(path, offset) {
  if (!path || offset <= 0) return true
  const handle = await open(path, 'r')
  try {
    const buffer = Buffer.alloc(1)
    const { bytesRead } = await handle.read(buffer, 0, 1, offset - 1)
    return bytesRead === 1 && buffer[0] === 0x0a
  } catch {
    return false
  } finally {
    await handle.close()
  }
}

// 读取文件 [fromOffset, size) 区间；文件被截断（size < fromOffset）时返回 invalid。
async function readFileRange(path, fromOffset) {
  const handle = await open(path, 'r')
  try {
    const { size } = await handle.stat()
    if (size < fromOffset) return { text: '', size, valid: false }
    const length = size - fromOffset
    const buffer = Buffer.alloc(length)
    await handle.read(buffer, 0, length, fromOffset)
    return { text: buffer.toString('utf8'), size, valid: true }
  } finally {
    await handle.close()
  }
}

/**
 * 扫描会话文件中的标签条目。
 *
 * 会话文件是追加写的 JSONL：全量扫描流式逐行读；传入 previous（上次扫描
 * 快照）且文件变大时只读新追加的字节，把扫描成本从整文件降到增量。
 *
 * 返回 { labels, scannedBytes, lastId, activeChain, assistantIds }：labels 是
 * 已解析的最终标签（含墓碑合并），其余字段是供下次增量扫描使用的快照。
 */
export async function scanSessionTreeLabels(path, options = {}) {
  const session = options.session || {}
  const previous = options.previous || null
  const tailScan = Boolean(previous)
  const fromOffset = tailScan ? Number(previous.scannedBytes) || 0 : 0
  if (!path) return { labels: [], scannedBytes: 0, lastId: '', activeChain: [], assistantIds: [] }

  const labelChanges = []
  const parentById = new Map()
  const newIds = new Set()
  const assistantIds = new Set(previous?.assistantIds || [])
  let header = null
  let name = ''
  let lastId = previous?.lastId || ''
  let fileSize = 0

  const field = (line, key) => {
    const needle = `"${key}":"`
    const start = line.indexOf(needle)
    if (start < 0) return null
    const from = start + needle.length
    const end = line.indexOf('"', from)
    return end < 0 ? null : line.slice(from, end)
  }

  const processLine = (line) => {
    if (!line) return
    if (line.startsWith('{"type":"session"')) {
      if (!header) {
        try {
          header = JSON.parse(line)
        } catch {}
      }
      return
    }
    if (line.startsWith('{"type":"session_info"')) {
      try {
        const entry = JSON.parse(line)
        if (entry.name) name = entry.name
      } catch {}
      return
    }
    if (line.startsWith('{"type":"label"')) {
      try {
        const entry = JSON.parse(line)
        // 标签条目也是树节点（appendLabelChange 会推进 leaf），其 id/parentId
        // 参与活动链重建，否则后续消息追加后链会断开。
        if (entry.id) {
          lastId = entry.id
          newIds.add(entry.id)
          parentById.set(entry.id, entry.parentId ?? null)
        }
        if (entry.targetId) {
          // 空 label 是删除墓碑（JSON.stringify 会丢掉 undefined 的 label 字段），
          // 必须收集进变更日志，最后一个条目才代表该节点的最终标签。
          labelChanges.push({
            entryId: entry.targetId,
            label: String(entry.label || ''),
            timestamp: entry.timestamp,
          })
        }
      } catch {}
      return
    }
    if (!line.startsWith('{')) return
    const id = field(line, 'id')
    if (!id) return
    lastId = id
    newIds.add(id)
    parentById.set(id, field(line, 'parentId'))
    if (line.startsWith('{"type":"message"') && line.includes('"role":"assistant"')) {
      assistantIds.add(id)
    }
  }

  if (tailScan) {
    const range = await readFileRange(path, fromOffset)
    fileSize = range.size
    // 文件变小（截断/重写）时增量边界不可信，退回全量扫描。
    if (!range.valid) {
      const full = await scanSessionTreeLabels(path, { session })
      full.scannedBytes = full.scannedBytes || fileSize
      return full
    }
    for (const line of range.text.split(/\r?\n/)) processLine(line)
  } else {
    fileSize = (await stat(path)).size
    const lines = createInterface({
      input: createReadStream(path, { encoding: 'utf8' }),
      crlfDelay: Infinity,
    })
    for await (const line of lines) processLine(line)
  }

  // 活动链：从 lastId 沿 parentId 走到根。增量扫描时新条目的父链只存在于
  // 新字节里，走到旧区域后应衔接上次快照的 activeChain；接不上说明文件被
  // 重写，退回全量扫描。
  let activeChain
  if (tailScan) {
    const chain = []
    let cursor = lastId
    let consistent = false
    while (cursor) {
      chain.push(cursor)
      const parent = parentById.get(cursor)
      if (!parent) {
        consistent = cursor === previous.lastId && newIds.size === 0
        activeChain = consistent ? previous.activeChain : null
        break
      }
      if (!newIds.has(parent)) {
        consistent = parent === previous.lastId
        activeChain = consistent ? chain.concat(previous.activeChain) : null
        break
      }
      cursor = parent
    }
    if (!activeChain) {
      return scanSessionTreeLabels(path, { session })
    }
  } else {
    const chain = []
    const seen = new Set()
    for (let cursor = lastId; cursor; cursor = parentById.get(cursor) || '') {
      if (seen.has(cursor)) break
      seen.add(cursor)
      chain.push(cursor)
    }
    activeChain = chain
  }

  // 同一节点的多次标记/删除按文件顺序最后写入者生效，空标签视为已删除。
  const labelsByTarget = new Map()
  for (const entry of previous?.entries || []) {
    labelsByTarget.set(entry.entryId, {
      entryId: entry.entryId,
      label: entry.label,
      timestamp: entry.nodeTimestamp,
    })
  }
  for (const labelEntry of labelChanges) labelsByTarget.set(labelEntry.entryId, labelEntry)
  const activeIds = new Set(activeChain)
  const iso = (value) => (value instanceof Date ? value.toISOString() : String(value || ''))
  const labels = [...labelsByTarget.values()]
    .filter((labelEntry) => Boolean(labelEntry.label) && assistantIds.has(labelEntry.entryId))
    .map((labelEntry) => ({
      sessionId: session.id || header?.id || '',
      sessionName: String(session.name || name || ''),
      sessionCreated: iso(session.created || header?.timestamp),
      entryId: labelEntry.entryId,
      label: labelEntry.label,
      nodeTimestamp: String(labelEntry.timestamp || ''),
      active: activeIds.has(labelEntry.entryId),
    }))
  return {
    labels,
    scannedBytes: fileSize,
    lastId,
    activeChain,
    assistantIds: [...assistantIds],
  }
}

export { TREE_NAVIGATION_CUSTOM_TYPE }
