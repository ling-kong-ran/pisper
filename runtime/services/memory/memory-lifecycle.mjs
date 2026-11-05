// 记忆的生命周期：到期、引用失效、长期不用、超保留期，以及恢复和撤销。接口先定好，实现见 M2。

// source_path 指向内容的哈希。没有路径或读不到时返回空字符串。
export function computeAnchorHash(sourcePath) {
  void sourcePath
  return ''
}

// 到期的转归档（不再物理删除）。其余规则见 M2。
export function sweepMemoryLifecycle(store) {
  const now = store.isoNow()
  store.db
    .prepare(
      `UPDATE memories SET status = 'archived', archived_at = ?, archive_reason = 'expired', updated_at = ?
       WHERE status IN ('active', 'stale') AND expires_at IS NOT NULL AND expires_at <= ?`,
    )
    .run(now, now, now)
  return { archived: 0, purged: 0 }
}

// 检索命中带 source_path 的事实时校验引用：文件没了或内容变了就标 stale。
export function checkMemoryAnchors(store, hits) {
  void store
  void hits
}

export function confirmMemory(store, id) {
  return store.getMemory(id)
}

export function restoreMemory(store, id) {
  return store.getMemory(id)
}

export function undoMemoryEvent(store, id) {
  void store
  void id
  return null
}
