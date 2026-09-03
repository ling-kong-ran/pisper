// Pi 会话流重试策略：只恢复明确的瞬时上游断流，不掩盖鉴权、配额或用户中止。
const AUTHENTICATION_ERROR_PATTERN =
  /\b(?:401|403|unauthori[sz]ed|forbidden|invalid\s+(?:api\s*key|token|credential)|authentication\s+(?:failed|required|error))\b/i
const TRANSIENT_STREAM_READ_ERROR_PATTERN =
  /\b(?:stream[_\s-]?read[_\s-]?error|upstream[_\s-]?error\s*:\s*(?:upstream\s+)?request\s+failed|econn(?:reset|aborted)|connection\s+(?:reset|closed|lost)|premature(?:ly)?\s+clos(?:e|ed)|incomplete\s+stream|too\s+many\s+pending\s+requests|rate\s+limit(?:ed)?|retry\s+later|stream\s+(?:ended|closed|interrupted)\s+before\b[\s\S]{0,120}\bterminal\b|upstream\b[\s\S]{0,120}\b(?:closed|interrupted)\b)/i
const PISPER_STREAM_RETRY_PATCH = Symbol('pisper.stream-retry-patch')

// 给会话打上瞬时流错误重试补丁：把上游明确的流中断交给 Pi 引擎的内置重试机制处理；同一会话只打一次补丁。
export function installTransientStreamRetry(session) {
  if (
    !session ||
    session[PISPER_STREAM_RETRY_PATCH] ||
    typeof session._isRetryableError !== 'function'
  )
    return session
  const isRetryableError = session._isRetryableError.bind(session)
  session._isRetryableError = (message) => {
    if (
      message?.stopReason === 'error' &&
      !AUTHENTICATION_ERROR_PATTERN.test(String(message.errorMessage || '')) &&
      TRANSIENT_STREAM_READ_ERROR_PATTERN.test(String(message.errorMessage || ''))
    )
      return true
    return isRetryableError(message)
  }
  session[PISPER_STREAM_RETRY_PATCH] = true
  return session
}

// 父 Agent 与后台 Agent 必须共享同一条 Pi 重试判定路径；否则同一种上游断流会只在父会话自动恢复。
export async function createSessionWithTransientStreamRetry(createSession, options) {
  const result = await createSession(options)
  installTransientStreamRetry(result?.session)
  return result
}
