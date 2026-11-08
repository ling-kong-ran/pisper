import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'

export type McpHostStatus = {
  enabled: boolean
  listening: boolean
  host: string
  port: number
  url: string
  error: string | null
  /** Runtime 给的稳定错误码，界面按它翻译；旧 Runtime 没有这个字段。 */
  errorCode: McpHostErrorCode | null
}

const MCP_HOST_ERROR_CODES = [
  'settings_unreadable',
  'sdk_unavailable',
  'port_in_use',
  'listen_failed',
] as const
export type McpHostErrorCode = (typeof MCP_HOST_ERROR_CODES)[number]

function hostErrorCode(value: unknown): McpHostErrorCode | null {
  return MCP_HOST_ERROR_CODES.find((code) => code === value) ?? null
}

export type McpHostCredentials = {
  url: string
  token: string
}

function parseRecord(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalidResponseError()
  return Object.fromEntries(Object.entries(value))
}

function parseStatus(value: unknown): McpHostStatus {
  const item = parseRecord(value)
  if (
    typeof item.enabled !== 'boolean' ||
    typeof item.listening !== 'boolean' ||
    typeof item.host !== 'string' ||
    typeof item.port !== 'number' ||
    !Number.isInteger(item.port) ||
    typeof item.url !== 'string' ||
    (item.error !== null && typeof item.error !== 'string')
  ) {
    throw invalidResponseError()
  }
  return {
    enabled: item.enabled,
    listening: item.listening,
    host: item.host,
    port: item.port,
    url: item.url,
    error: item.error,
    errorCode: hostErrorCode(item.errorCode),
  }
}

function parseCredentials(value: unknown): McpHostCredentials {
  const item = parseRecord(value)
  if (typeof item.url !== 'string' || !item.url || typeof item.token !== 'string' || !item.token)
    throw invalidResponseError()
  return { url: item.url, token: item.token }
}

export const mcpHostApi = {
  status: (signal?: AbortSignal) => requestJson('/api/mcp-host', { signal, parse: parseStatus }),
  setEnabled: (enabled: boolean) =>
    requestJson('/api/mcp-host', {
      method: 'PATCH',
      body: { enabled },
      parse: parseStatus,
    }),
  credentials: (signal?: AbortSignal) =>
    requestJson('/api/mcp-host/credentials', {
      method: 'POST',
      body: {},
      signal,
      parse: parseCredentials,
    }),
  rotateToken: (signal?: AbortSignal) =>
    requestJson('/api/mcp-host/rotate-token', {
      method: 'POST',
      body: {},
      signal,
      parse: parseCredentials,
    }),
}
