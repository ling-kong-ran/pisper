// Plan（计划）在 payload 中的读取协议：兼容一个历史版本的 taskList 字段。
// undefined 表示负载里根本没提 plan（不可知），null 表示显式清空——
// 调用方依赖这个三分语义决定是否回退到其他来源，不要简化成布尔判断。
import type { EntityRecord, Plan } from '@/types/chat'

/**
 * Read the canonical plan field while accepting one release of taskList payloads.
 * `undefined` means the payload did not mention a plan; `null` means it explicitly cleared it.
 */
export function planFromPayload(payload: EntityRecord | null | undefined): Plan | null | undefined {
  if (!payload || typeof payload !== 'object') return undefined
  if (Object.hasOwn(payload, 'plan')) return (payload.plan as Plan | null | undefined) ?? null
  if (Object.hasOwn(payload, 'taskList')) {
    return (payload.taskList as Plan | null | undefined) ?? null
  }
  return undefined
}

export function planFromPayloadOr(
  payload: EntityRecord | null | undefined,
  fallback: Plan | null,
): Plan | null {
  const plan = planFromPayload(payload)
  return plan === undefined ? fallback : plan
}

export function isPlanUpdateEvent(event: string) {
  return event === 'plan_update' || event === 'task_list_update'
}

export function isPlanReadTool(name: unknown) {
  return name === 'get_plan' || name === 'get_task_list'
}

export function isPlanWriteTool(name: unknown) {
  return name === 'update_plan' || name === 'update_task_list'
}

export function isPlanTool(name: unknown) {
  return isPlanReadTool(name) || isPlanWriteTool(name)
}

export function planFromActivity(activity: EntityRecord | null | undefined) {
  return planFromPayload(activity)
}
