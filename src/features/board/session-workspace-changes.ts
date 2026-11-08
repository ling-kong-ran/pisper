// @public 会话协作提示变化通知：看板事件流里的 session_workspace 帧（同一文件夹里别的会话留言、改文件）
// 被接受后通知监听者；断线重连收到 snapshot 时通知 id '*'（期间可能漏掉了事件）。
// 空闲的会话页不轮询 /live，会话页据此重读，留言不用刷新页面就出现。
// 这个文件不导入看板数据层；帧来自 Runtime，按不可信 unknown 解析，非法帧丢弃不抛错。

export type SessionWorkspaceListener = (sessionId: string) => void

const listeners = new Set<SessionWorkspaceListener>()

export function parseSessionWorkspaceChange(data: unknown): string | null {
  if (data === null || typeof data !== 'object' || Array.isArray(data)) return null
  const id = (data as Record<string, unknown>).id
  return typeof id === 'string' && id && id.length <= 200 ? id : null
}

/** 订阅会话协作提示变化；返回取消订阅函数。 */
export function subscribeSessionWorkspaceChanges(listener: SessionWorkspaceListener) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** 只由看板 store 在帧被接受后调用。单个监听者抛错不影响其他监听者与事件流本身。 */
export function notifySessionWorkspaceFeedEvent(event: string, data: unknown, accepted: boolean) {
  if (!accepted || !listeners.size) return
  const id =
    event === 'snapshot'
      ? '*'
      : event === 'session_workspace'
        ? parseSessionWorkspaceChange(data)
        : null
  if (!id) return
  for (const listener of [...listeners]) {
    try {
      listener(id)
    } catch {
      // 监听者自己的错误不能打断看板事件流的处理。
    }
  }
}
