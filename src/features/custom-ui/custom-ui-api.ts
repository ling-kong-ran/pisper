// 自定义 UI 组件的类型与 API 封装：与 runtime/services/custom-ui-service.mjs
// 的清单输出保持契约一致。目录来自 Runtime；用户主目录按服务端平台缩写为 ~ 或 %USERPROFILE%。
import { apiJson } from '@/lib/api'

export type CustomUiComponent = {
  id: string
  name: string
  version: string
  description: string
  entry: string
  permissions: string[]
  entryUrl: string
  directory: string
}

export type CustomUiComponentsData = {
  root: string
  components: CustomUiComponent[]
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function isComponent(value: unknown): value is CustomUiComponent {
  return (
    isRecord(value) &&
    ['id', 'name', 'version', 'description', 'entry', 'entryUrl', 'directory'].every(
      (key) => typeof value[key] === 'string',
    ) &&
    Array.isArray(value.permissions) &&
    value.permissions.every((item) => typeof item === 'string')
  )
}

export async function listCustomUiComponents(
  signal?: AbortSignal,
): Promise<CustomUiComponentsData> {
  const data = await apiJson<unknown>('/api/custom-ui/components', { signal })
  if (
    !isRecord(data) ||
    typeof data.root !== 'string' ||
    !Array.isArray(data.components) ||
    !data.components.every(isComponent)
  ) {
    throw new Error('Invalid custom UI component response')
  }
  return { root: data.root, components: data.components }
}

export type CustomUiView = { id: string; entryUrl: string }

export async function createCustomUiView(
  componentId: string,
  signal: AbortSignal,
): Promise<CustomUiView> {
  const data = await apiJson<unknown>(
    `/api/custom-ui/components/${encodeURIComponent(componentId)}/views`,
    { method: 'POST', body: { origin: window.location.origin }, signal },
  )
  if (
    !isRecord(data) ||
    typeof data.id !== 'string' ||
    !/^[a-f0-9]{64}$/.test(data.id) ||
    typeof data.entryUrl !== 'string' ||
    !data.entryUrl.startsWith(`/api/custom-ui/render/${data.id}/assets/`)
  ) {
    throw new Error('Invalid custom UI view response')
  }
  return { id: data.id, entryUrl: data.entryUrl }
}

export function renewCustomUiView(id: string, signal: AbortSignal) {
  return apiJson(`/api/custom-ui/views/${encodeURIComponent(id)}`, { method: 'PUT', signal })
}

export function releaseCustomUiView(id: string) {
  return apiJson(`/api/custom-ui/views/${encodeURIComponent(id)}`, {
    method: 'DELETE',
    keepalive: true,
  })
}
