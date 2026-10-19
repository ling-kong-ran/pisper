// @public 应用壳打开当前焦点任务资源的轻量契约，不导入工作台组件或启动副作用。
export const TASK_CONTENT_REQUESTED = 'pisper:task-content-requested'
export const taskContentKinds = ['changes', 'terminal', 'processes', 'sources', 'browser'] as const
export type TaskContentKind = (typeof taskContentKinds)[number]
export function requestTaskContent(kind: TaskContentKind) {
  window.dispatchEvent(new CustomEvent(TASK_CONTENT_REQUESTED, { detail: { kind } }))
}
export function parseTaskContentRequest(value: unknown): TaskContentKind | null {
  if (!value || typeof value !== 'object' || !('kind' in value)) return null
  return taskContentKinds.find((kind) => kind === value.kind) || null
}
