// 后台整理：L2 时间摘要、L3 画像、合并重复。接口先定好，实现见 M4。

/**
 * 读 L3 画像，拼成注入系统提示的文本。没有内容时返回空字符串。
 * @param {object} store
 * @param {string[]} spaceIds
 * @returns {string}
 */
export function loadProfileContext(store, spaceIds) {
  void store
  void spaceIds
  return ''
}

/**
 * 按 id 展开：事实 → 证据、版本链、原文；摘要 → 子事实；原文 → 所属会话。
 * @returns {object | null}
 */
export function openMemoryDocument(store, id) {
  const memory = store.getMemory(id)
  return memory ? { kind: 'fact', item: memory } : null
}

/**
 * @param {object} store
 * @param {{ force?: boolean, signal?: AbortSignal }} [options]
 */
export async function runMemoryConsolidation(store, options = {}) {
  void store
  void options
  return { ran: false }
}
