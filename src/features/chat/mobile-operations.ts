// 移动设备操作客户端：消费 Runtime SSE 请求，调用 Tauri 原生桥后把结果
// 回传到同一会话。操作 ID 去重，避免流重放触发第二次系统操作。
import { chatApi, type ApiRecord } from './chat-api'

const ALLOWED_OPERATIONS = new Set([
  'contacts.search',
  'camera.capture',
  'location.current',
  'device.info',
  'device.capabilities',
  'device.battery',
  'device.storage',
  'device.memory',
  'device.network',
  'device.display',
  'device.locale',
  'device.status',
  'device.clipboard.get',
  'device.clipboard.set',
  'device.vibrate',
  'device.flashlight',
  'device.notify',
  'photos.list',
  'photos.create_album',
  'photos.add_to_album',
  'photos.delete',
  'apps.open_url',
  'apps.open_map',
  'apps.open_system_settings',
  'apps.open_dialer',
  'apps.compose_sms',
  'apps.open_app',
  'apps.share_text',
])
const inFlight = new Map<string, Promise<void>>()
const completed: string[] = []

function invokeMobile<T>(command: string, args?: unknown): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('native bridge unavailable'))
  return invoke<T>(command, args)
}

async function execute(sessionId: string, data: ApiRecord) {
  const id = String(data.id || '')
  const operation = String(data.operation || '')
  if (!id || !ALLOWED_OPERATIONS.has(operation)) return
  const expiresAt = Date.parse(String(data.expiresAt || ''))
  if (Number.isFinite(expiresAt) && expiresAt <= Date.now()) {
    await chatApi.resolveMobileOperation(sessionId, id, {
      ok: false,
      error: '移动设备操作请求已过期。',
    })
    return
  }
  try {
    const result = await invokeMobile<ApiRecord>('mobile_execute_device_operation', {
      request: {
        id,
        operation,
        parameters:
          data.parameters && typeof data.parameters === 'object' ? data.parameters : undefined,
      },
    })
    await chatApi.resolveMobileOperation(sessionId, id, { ok: true, result })
  } catch (cause) {
    await chatApi.resolveMobileOperation(sessionId, id, {
      ok: false,
      error: cause instanceof Error ? cause.message : String(cause),
    })
  } finally {
    completed.push(id)
    if (completed.length > 128) completed.splice(0, completed.length - 128)
  }
}

export function handleMobileOperationCancellation(data: ApiRecord) {
  const id = String(data.id || '')
  if (!id || completed.includes(id)) return
  completed.push(id)
  if (completed.length > 128) completed.splice(0, completed.length - 128)
}

export function handleMobileOperationRequest(sessionId: string, data: ApiRecord) {
  const id = String(data.id || '')
  if (!id || completed.includes(id) || inFlight.has(id)) return
  const pending = execute(sessionId, data)
    .catch(() => {})
    .finally(() => inFlight.delete(id))
  inFlight.set(id, pending)
}
