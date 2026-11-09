// 会话列表里的“最近一条”：历史会话页在标题下显示最后一次对话，不用打开会话就知道停在哪里。
// Pi 的会话目录只给首条消息；这里从会话文件尾部倒着找最后一条有文字的用户 / 助手消息，
// 按文件路径 + 修改时间 + 消息数缓存。读文件在后台进行：列表接口不等它，下一次读列表时带上。
import { open } from 'node:fs/promises'
import {
  ATTACHMENT_MARKER,
  isInternalParentMessage,
  textFromContent,
} from './stream-projection.mjs'

export const LAST_MESSAGE_MAX_CHARS = 160
// 先读 64 KiB；最后几行是很大的工具结果时再放宽，超过上限就当没有，不为一行摘要读完整个文件。
const TAIL_BYTES = [64 * 1024, 1024 * 1024]
const MAX_CACHED = 5000

// 一行摘要：去掉 Markdown 记号、代码块和多余空白，截到上限。
export function compactPreview(text) {
  const plain = String(text || '')
    .replace(/```[\s\S]*?(```|$)/g, ' ')
    .replace(/^\s{0,3}(#{1,6}|>|[-*+]|\d+[.)])\s+/gm, '')
    .replace(/[*_`~]+/g, '')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\s+/g, ' ')
    .trim()
  return plain.length > LAST_MESSAGE_MAX_CHARS
    ? `${plain.slice(0, LAST_MESSAGE_MAX_CHARS - 1).trimEnd()}…`
    : plain
}

// 用户消息去掉 Pisper 注入的附件上下文；目标延续、Agent 完成等内部消息不算用户说的话。
export function messagePreview(message) {
  if (message?.role !== 'user' && message?.role !== 'assistant') return null
  let text = textFromContent(message.content)
  if (message.role === 'user') {
    if (isInternalParentMessage(text)) return null
    const marker = text.indexOf(ATTACHMENT_MARKER)
    if (marker >= 0) text = text.slice(0, marker)
  }
  const preview = compactPreview(text)
  return preview ? { role: message.role, text: preview } : null
}

export function lastMessageFromMessages(messages) {
  const list = Array.isArray(messages) ? messages : []
  for (let index = list.length - 1; index >= 0; index -= 1) {
    const preview = messagePreview(list[index])
    if (preview) return preview
  }
  return null
}

export async function readLastMessage(path) {
  const handle = await open(path, 'r')
  try {
    const { size } = await handle.stat()
    for (const limit of TAIL_BYTES) {
      const length = Math.min(size, limit)
      const buffer = Buffer.alloc(length)
      await handle.read(buffer, 0, length, size - length)
      const lines = buffer.toString('utf8').split('\n')
      // 没读到文件开头时第一行是半截，丢掉。
      if (length < size) lines.shift()
      for (let index = lines.length - 1; index >= 0; index -= 1) {
        if (!lines[index].includes('"message"')) continue
        let entry
        try {
          entry = JSON.parse(lines[index])
        } catch {
          continue
        }
        const preview = entry?.type === 'message' ? messagePreview(entry.message) : null
        if (preview) return preview
      }
      if (length === size) break
    }
    return null
  } finally {
    await handle.close()
  }
}

// get(info) 立即返回已缓存的结果（文件变了也先返回上一版），没有或过期时排队在后台重读。
export function createLastMessageCache({ read = readLastMessage, concurrency = 4 } = {}) {
  const cache = new Map()
  const queue = []
  const queued = new Set()
  let running = 0

  const versionOf = (info) => `${new Date(info.modified).getTime()}:${info.messageCount ?? ''}`

  const pump = () => {
    while (running < concurrency && queue.length) {
      const { path, version } = queue.shift()
      running += 1
      Promise.resolve()
        .then(() => read(path))
        .catch(() => null)
        .then((value) => {
          cache.delete(path)
          cache.set(path, { version, value })
          while (cache.size > MAX_CACHED) cache.delete(cache.keys().next().value)
        })
        .finally(() => {
          running -= 1
          queued.delete(path)
          pump()
        })
    }
  }

  return {
    get(info) {
      const path = typeof info?.path === 'string' ? info.path : ''
      if (!path) return null
      const entry = cache.get(path)
      const version = versionOf(info)
      if (entry?.version !== version && !queued.has(path)) {
        queued.add(path)
        queue.push({ path, version })
        pump()
      }
      return entry?.value ?? null
    },
  }
}
