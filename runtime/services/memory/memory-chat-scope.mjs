// 无项目聊天共用一个管理空间。会话检索在各路召回的 LIMIT 之前按 session_id 过滤；
// 记忆管理页显式列出空间，不传 sessionId，可以浏览用户自己的所有聊天。
export function chatSessionFilter(spaceIds, sessionId, table = '') {
  if (sessionId == null || !spaceIds.includes('chats')) return { sql: '', args: [] }
  const prefix = table ? `${table}.` : ''
  return {
    sql: ` AND (${prefix}space_id <> 'chats' OR ${prefix}session_id = ?)`,
    args: [sessionId],
  }
}

// 无项目会话里的任务没有宿主项目。只有有用户原话依据的通用偏好可以自动跨聊天共享。
export function memoryWriteSpace(workspaceId, { scope, type }, userEvidence) {
  if (scope !== 'global') return workspaceId
  if (workspaceId === 'chats' && (type !== 'preference' || !userEvidence)) return workspaceId
  return 'global'
}

export function sameMemoryScope(left, right) {
  return (
    left.space_id === right.space_id &&
    (left.space_id !== 'chats' || left.session_id === right.session_id)
  )
}
