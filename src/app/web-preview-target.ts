// 外链预览要落到哪个会话：只认会话目录缓存里存在且未归档的会话。
// 存着的活动会话 id 可能早已删除或归档；返回空串表示没有可用会话，调用方改为在外部浏览器打开。
export type CachedSessions = { sessions?: Array<{ id?: unknown; archived?: unknown }> }

export function previewSessionTarget(storedId: string | null, data: CachedSessions | undefined) {
  if (!storedId) return ''
  const session = data?.sessions?.find((item) => item?.id === storedId)
  return session && session.archived !== true ? storedId : ''
}
