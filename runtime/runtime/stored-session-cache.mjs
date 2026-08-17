/**
 * 存储会话缓存（storedSessionsCache）的增量更新工具。
 *
 * listStoredSessions 的缓存是整个会话数组；新建/物化单个会话时如果触发
 * 全量重扫（refresh: true），SessionManager.listAll 会逐行读完目录下每个
 * .jsonl 文件，会话多、文件大时代价很高。这里提供单条会话的增量插入，
 * 让新会话立即可见而无需重扫磁盘。
 */
export function upsertStoredSessionCache(cache, info) {
  if (!cache) return
  const index = cache.findIndex((session) => session?.id === info.id)
  if (index >= 0) cache.splice(index, 1, info)
  else cache.push(info)
  cache.sort(
    (left, right) => new Date(right.modified).getTime() - new Date(left.modified).getTime(),
  )
  return cache
}
