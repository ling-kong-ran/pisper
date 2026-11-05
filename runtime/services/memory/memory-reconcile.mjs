// 写入对账：新事实和已有事实矛盾时，让辅助模型判定要不要替换旧事实。
// 接口先定好，实现见 M2。
/**
 * 处理 reconcile_status = 'pending' 的事实。没有辅助模型时什么都不做（读取时的时间排序兜底）。
 * @param {object} store createStore() 给的窄接口
 * @param {{ signal?: AbortSignal, limit?: number }} [options]
 * @returns {Promise<{ processed: number }>}
 */
export async function reconcilePendingMemories(store, options = {}) {
  void store
  void options
  return { processed: 0 }
}
