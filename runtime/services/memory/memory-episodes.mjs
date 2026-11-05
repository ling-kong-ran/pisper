// L0 原文层：每轮对话脱敏后存一份，用来跨会话检索和后台批量抽取。接口先定好，实现见 M3。

/**
 * @param {object} store
 * @param {{ sessionId: string, cwd?: string, spaceId?: string, entryId?: string, user: string, assistant: string, at?: string }} input
 * @returns {{ id: string } | null}
 */
export function recordEpisode(store, input) {
  void store
  void input
  return null
}

/**
 * 需要批量抽取的会话：游标之后有新原文，且会话已空闲或积累够多。
 * @returns {Array<{ sessionId: string, cwd: string, spaceId: string, episodes: Array<object> }>}
 */
export function pendingEpisodeBatches(store, options = {}) {
  void store
  void options
  return []
}

export function saveSessionCursor(store, sessionId, episodeAt) {
  void store
  void sessionId
  void episodeAt
}

export function deleteSessionEpisodes(store, sessionId) {
  void store
  void sessionId
  return 0
}
