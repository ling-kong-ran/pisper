// @public 组件变化通知（计划 §3.10、§4.1）：看板事件流里的 component_changed 帧被接受后通知监听者，
// 断线重连收到 snapshot 时再通知一次 id '*'（期间可能漏掉了组件事件，监听者应作废全部组件查询）。
// 这个文件不导入看板数据层：组件宿主的轻量入口只需要订阅，不应因此把看板 store 带进自己的 chunk。
// 帧来自 Runtime，按不可信 unknown 解析；非法帧直接丢弃，不抛错。

export type ComponentChangeReason = 'files' | 'state' | 'instances' | 'deleted' | 'created'

export type ComponentChange = {
  // 组件 id；'*' 表示“全部作废”（重连快照）。
  id: string
  rev?: string
  reason?: ComponentChangeReason
}

export type ComponentChangeListener = (change: ComponentChange) => void

const REASONS: ReadonlySet<string> = new Set(['files', 'state', 'instances', 'deleted', 'created'])
// 与 Runtime 的组件 id 规则一致（shared/custom-ui-manifest.mjs 的 COMPONENT_ID_PATTERN）；
// 这里不导入 shared 模块，保持文件零依赖。
const COMPONENT_ID = /^[a-z0-9][a-z0-9._-]{0,63}$/
const REV = /^[a-f0-9]{24}$/

/** 解析 component_changed 帧的 data；不合法时返回 null。 */
export function parseComponentChange(data: unknown): ComponentChange | null {
  if (data === null || typeof data !== 'object' || Array.isArray(data)) return null
  const frame = data as Record<string, unknown>
  if (typeof frame.id !== 'string' || !COMPONENT_ID.test(frame.id)) return null
  const change: ComponentChange = { id: frame.id }
  if (typeof frame.rev === 'string' && REV.test(frame.rev)) change.rev = frame.rev
  if (typeof frame.reason === 'string' && REASONS.has(frame.reason))
    change.reason = frame.reason as ComponentChangeReason
  return change
}

const listeners = new Set<ComponentChangeListener>()

/** 订阅组件变化；返回取消订阅函数。 */
export function subscribeComponentChanges(listener: ComponentChangeListener): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** 只由看板 store 在帧被接受后调用。单个监听者抛错不影响其他监听者与事件流本身。 */
export function emitComponentChange(change: ComponentChange) {
  for (const listener of [...listeners]) {
    try {
      listener(change)
    } catch {
      // 监听者自己的错误不能打断看板事件流的处理。
    }
  }
}

/** 看板 store 的接线：component_changed 被接受时转发解析结果，snapshot 被接受时通知 '*'。 */
export function notifyComponentFeedEvent(event: string, data: unknown, accepted: boolean) {
  if (!accepted || !listeners.size) return
  if (event === 'snapshot') emitComponentChange({ id: '*' })
  else if (event === 'component_changed') {
    const change = parseComponentChange(data)
    if (change) emitComponentChange(change)
  }
}
