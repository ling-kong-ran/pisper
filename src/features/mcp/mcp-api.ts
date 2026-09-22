import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalidResponseError()
  return Object.fromEntries(Object.entries(value))
}

function text(value: unknown, required = false): string {
  if (value == null && !required) return ''
  if (typeof value !== 'string' || (required && !value)) throw invalidResponseError()
  return value
}

function number(value: unknown, fallback = 0): number {
  if (value == null) return fallback
  if (typeof value !== 'number' || !Number.isFinite(value)) throw invalidResponseError()
  return value
}

function boolean(value: unknown): boolean {
  if (typeof value !== 'boolean') throw invalidResponseError()
  return value
}

function list<T>(value: unknown, parse: (item: unknown) => T): T[] {
  if (value === undefined) return []
  if (!Array.isArray(value)) throw invalidResponseError()
  return value.map(parse)
}

function service(value: unknown) {
  const item = record(value)
  return {
    id: text(item.id, true),
    name: text(item.name, true),
    status: text(item.status),
    transport: text(item.transport),
    endpoint: text(item.endpoint),
    command: text(item.command),
    workingDirectory: text(item.workingDirectory),
    enabled: boolean(item.enabled),
    error: text(item.error),
    latencyMs: item.latencyMs == null ? null : number(item.latencyMs),
    lastPingAt: text(item.lastPingAt),
    auth: text(item.auth),
    authCount: number(item.authCount),
  }
}

function tool(value: unknown) {
  const item = record(value)
  return {
    serviceId: text(item.serviceId, true),
    serviceName: text(item.serviceName),
    name: text(item.name, true),
    piName: text(item.piName, true),
    description: text(item.description),
    enabled: boolean(item.enabled),
    serviceEnabled: boolean(item.serviceEnabled),
    risk: text(item.risk),
  }
}

function call(value: unknown) {
  const item = record(value)
  return {
    id: text(item.id, true),
    serviceId: text(item.serviceId, true),
    toolName: text(item.toolName),
    timestamp: text(item.timestamp),
    status: text(item.status),
    error: text(item.error),
    durationMs: number(item.durationMs),
  }
}

export function parseMcpDashboard(value: unknown) {
  const data = record(value)
  if (!Array.isArray(data.services)) throw invalidResponseError()
  const metrics = data.metrics === undefined ? {} : record(data.metrics)
  return {
    services: data.services.map(service),
    tools: list(data.tools, tool),
    calls: list(data.calls, call),
    metrics: {
      totalServices: number(metrics.totalServices),
      onlineServices: number(metrics.onlineServices),
      availableTools: number(metrics.availableTools),
      restrictedTools: number(metrics.restrictedTools),
      errorRate: number(metrics.errorRate),
    },
  }
}

export type McpDashboard = ReturnType<typeof parseMcpDashboard>
export type McpService = McpDashboard['services'][number]
export type McpTool = McpDashboard['tools'][number]

const serverPath = (id: string) => `/api/mcp/${encodeURIComponent(id)}`

export const mcpApi = {
  // 观察状态不触发连接；刷新/测试由用户动作发起，避免轮询重启 stdio 进程。
  dashboard: (signal?: AbortSignal) =>
    requestJson('/api/mcp?refresh=0', { signal, parse: parseMcpDashboard }),
  add: (spec: string) =>
    requestJson('/api/mcp', { method: 'POST', body: { spec }, parse: parseMcpDashboard }),
  setServerEnabled: (id: string, enabled: boolean) =>
    requestJson(serverPath(id), { method: 'PATCH', body: { enabled }, parse: parseMcpDashboard }),
  setToolEnabled: (serviceId: string, name: string, enabled: boolean) =>
    requestJson(`${serverPath(serviceId)}/tools/${encodeURIComponent(name)}`, {
      method: 'PATCH',
      body: { enabled },
      parse: parseMcpDashboard,
    }),
  test: (id: string) =>
    requestJson(`${serverPath(id)}/test`, { method: 'POST', body: {}, parse: parseMcpDashboard }),
  remove: async (id: string) => {
    await requestJson(serverPath(id), { method: 'DELETE' })
    return mcpApi.dashboard()
  },
}
