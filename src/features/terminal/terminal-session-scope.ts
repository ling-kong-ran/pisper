// 会话终端作用域：记录每个终端绑定到的会话，孤儿终端（会话已关）标记。
export type SessionTerminal = {
  id: string
  sessionId: string
  orphaned: boolean
}

export function visibleSessionTerminals<T extends SessionTerminal>(
  terminals: T[],
  activeSessionId: string,
) {
  return terminals.filter((terminal) => terminal.sessionId === activeSessionId)
}

export function activeSessionTerminalId<T extends SessionTerminal>(
  terminals: T[],
  activeIds: Record<string, string>,
  activeSessionId: string,
) {
  const visible = visibleSessionTerminals(terminals, activeSessionId)
  const requestedId = activeIds[activeSessionId] || ''
  return visible.some((terminal) => terminal.id === requestedId)
    ? requestedId
    : visible[0]?.id || ''
}

export function markOrphanedSessionTerminals<T extends SessionTerminal>(
  terminals: T[],
  sessionIds: Iterable<string>,
  preferredSessionId: string,
) {
  const knownIds = new Set(sessionIds)
  const fallbackSessionId = knownIds.has(preferredSessionId)
    ? preferredSessionId
    : knownIds.values().next().value || ''
  let changed = false
  const next = terminals.map((terminal) => {
    if (terminal.sessionId && knownIds.has(terminal.sessionId)) return terminal
    if (!terminal.sessionId && !terminal.orphaned) return terminal
    if (terminal.sessionId === fallbackSessionId) return terminal
    changed = true
    return { ...terminal, sessionId: fallbackSessionId, orphaned: true }
  })
  return changed ? next : terminals
}
