// 思考用时（A9，代理信息行"思考 6.2s"）：引擎的会话文件不记录思考时长，Runtime 在运行中计时，
// 按"这一轮最后一条助手消息的时间戳"记进会话元数据 thinkingDurations（只做加法），
// 转录投影时再贴回对应的代理回复。每个会话最多保留最近 200 条。
export const MAX_THINKING_DURATIONS = 200

/** @param {Record<string, any>} sessionMeta @param {string} sessionId @param {unknown} timestamp @param {number} ms */
export function recordThinkingDuration(sessionMeta, sessionId, timestamp, ms) {
  const key = String(timestamp ?? '')
  const value = Math.round(Number(ms))
  if (!sessionId || !key || !Number.isFinite(value) || value <= 0) return
  const meta = sessionMeta[sessionId] || {}
  const durations = { ...(isRecord(meta.thinkingDurations) ? meta.thinkingDurations : {}) }
  delete durations[key]
  durations[key] = value
  const keys = Object.keys(durations)
  for (const stale of keys.slice(0, Math.max(0, keys.length - MAX_THINKING_DURATIONS)))
    delete durations[stale]
  sessionMeta[sessionId] = { ...meta, thinkingDurations: durations }
}

/** 把思考用时贴到转录里的代理回复上；没有可贴的就原样返回同一个数组。 */
export function attachThinkingDurations(messages, durations) {
  if (!Array.isArray(messages) || !isRecord(durations)) return messages
  let changed = false
  const result = messages.map((message) => {
    if (message?.role !== 'agent' || message.timestamp == null) return message
    const ms = durations[String(message.timestamp)]
    if (!Number.isFinite(ms) || ms <= 0) return message
    changed = true
    return { ...message, thinkingMs: ms }
  })
  return changed ? result : messages
}

function isRecord(value) {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}
