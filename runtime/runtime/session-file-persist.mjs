// 新会话文件强制落盘（语义与理由见 ensureSessionFilePersisted 的实现注释）。
import { mkdir, stat, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { dirname } from 'node:path'

const pendingPersistence = new WeakMap()

// Pi persists a session file lazily: nothing hits disk until the first
// assistant message arrives. A fresh conversation interrupted before the model
// replied therefore has no file, so releasing its resident runtime (forced
// interruption, idle sweep) makes the session vanish from disk lookups and a
// workspace switch fails with "session not found". Write the minimal valid
// file (session header + session_info, CURRENT_SESSION_VERSION = 3) at
// materialization so the session stays addressable and recoverable.
export function ensureSessionFilePersisted(sessionManager, name = '', cwd = '') {
  if (!sessionManager) return Promise.resolve()
  const existing = pendingPersistence.get(sessionManager)
  if (existing) return existing
  const operation = persistSessionFile(sessionManager, name, cwd)
  pendingPersistence.set(sessionManager, operation)
  void operation
    .finally(() => {
      if (pendingPersistence.get(sessionManager) === operation)
        pendingPersistence.delete(sessionManager)
    })
    .catch(() => {})
  return operation
}

async function persistSessionFile(sessionManager, name, cwd) {
  const file = sessionManager?.sessionFile
  if (!file) return
  const exists = await stat(file)
    .then(() => true)
    .catch(() => false)
  if (exists) return
  const sessionId = sessionManager.getSessionId()
  const timestamp = new Date().toISOString()
  const resolvedCwd = cwd || sessionManager.getCwd?.() || ''
  const cleanName = String(name || '')
    .replace(/[\r\n]+/g, ' ')
    .trim()
  const lines = [
    JSON.stringify({ type: 'session', version: 3, id: sessionId, timestamp, cwd: resolvedCwd }),
    JSON.stringify({
      type: 'session_info',
      id: randomUUID().slice(0, 8),
      parentId: null,
      timestamp,
      name: cleanName || 'New conversation',
    }),
  ]
  await mkdir(dirname(file), { recursive: true })
  try {
    await writeFile(file, `${lines.map((line) => `${line}\n`).join('')}`, { flag: 'wx' })
  } catch (error) {
    // 另一个管理者刚创建了同一会话文件时，不得截断其内容。
    if (error?.code !== 'EEXIST') throw error
  }
  // The manager still considers a new session unflushed and would try to
  // recreate this file with `wx` when the first assistant message arrives.
  // Reloading the file synchronizes its in-memory entries and persistence state.
  sessionManager.setSessionFile(file)
}
