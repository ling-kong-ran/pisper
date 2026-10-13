// 聊天入口只需要工具目录；请求与响应校验由插件领域持有。
import { apiJson } from '@/lib/http/api'

type CatalogTool = { name: string; id?: string; label?: string; description: string }

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('Invalid session plugin catalog')
  return value as Record<string, unknown>
}

function optionalText(value: unknown): string | undefined {
  if (value === undefined) return undefined
  if (typeof value !== 'string') throw new Error('Invalid session plugin catalog')
  return value
}

export async function getSessionPluginCatalog(sessionId: string, signal?: AbortSignal) {
  const data = record(
    await apiJson<unknown>(`/api/plugins?sessionId=${encodeURIComponent(sessionId)}`, { signal }),
  )
  const callable = data.callableToolNames ?? []
  if (
    !Array.isArray(data.plugins) ||
    !Array.isArray(callable) ||
    !callable.every((name): name is string => typeof name === 'string')
  )
    throw new Error('Invalid session plugin catalog')
  const tools = data.plugins.flatMap((value): CatalogTool[] => {
    const plugin = record(value)
    if (!Array.isArray(plugin.capabilities)) throw new Error('Invalid session plugin catalog')
    return plugin.capabilities.map((value): CatalogTool => {
      const tool = record(value)
      if (typeof tool.name !== 'string') throw new Error('Invalid session plugin catalog')
      return {
        name: tool.name,
        id: optionalText(tool.id),
        label: optionalText(tool.label),
        description: optionalText(tool.description) ?? '',
      }
    })
  })
  return { tools, callableToolNames: callable }
}
