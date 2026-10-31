// 组件信任与生命周期（A8，规格 5.4、5.7、5.9）的请求与解析：版本 / 对比 / 回滚、导出 ZIP、回收站、
// 导入暂存与信任确认、能力台账与撤销。只有工作室（studio/*）与宿主错误卡用，不进放置轻量入口。
// 响应一律按 unknown 校验后再交给界面；字段与 runtime/services/custom-ui-service.mjs 的输出一致。
import type { CapabilityType, NativeManifest } from '@shared/custom-ui-manifest.mjs'
import { ApiError, requestBlob, requestJson } from '@/lib/http'
import {
  parseComponentFiles,
  parseComponentRow,
  parseComponentScan,
  parseFieldErrors,
  parseNativeManifest,
  parseStagedComponent,
  type ComponentFieldError,
  type ComponentFileEntry,
  type ComponentRow,
  type ComponentScan,
  type StagedComponent,
} from './custom-ui-api'

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function text(value: unknown, max = 4000): string {
  return typeof value === 'string' ? value.slice(0, max) : ''
}

function integer(value: unknown) {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : 0
}

const componentPath = (id: string) => `/api/custom-ui/components/${encodeURIComponent(id)}`
const CAPABILITIES: readonly CapabilityType[] = [
  'network',
  'notify',
  'sessions.read',
  'sessions.open',
  'storage',
]

function rowResponse(value: unknown): ComponentRow {
  const row = isRecord(value) ? parseComponentRow(value.component) : null
  if (!row) throw new Error('Invalid custom UI component response')
  return row
}

// —— 版本（规格 5.9） ——

export type VersionSource = 'agent' | 'import' | 'restore' | 'local'
export type VersionFileChange = { path: string; added: number; removed: number }
export type ComponentVersion = {
  n: number
  // 这一版写入的时间；更早的记录被裁掉时为 null。
  at: string | null
  summary: string
  source: VersionSource | null
  files: VersionFileChange[]
  deleted: string[]
}
export type ComponentVersions = {
  current: ComponentVersion
  versions: ComponentVersion[]
  max: number
}

function parseVersion(value: unknown): ComponentVersion | null {
  if (!isRecord(value) || !Number.isSafeInteger(value.n) || Number(value.n) < 1) return null
  const sources: readonly VersionSource[] = ['agent', 'import', 'restore', 'local']
  return {
    n: Number(value.n),
    at: typeof value.at === 'string' ? value.at : null,
    summary: text(value.summary, 200),
    source: sources.includes(value.source as VersionSource)
      ? (value.source as VersionSource)
      : null,
    files: (Array.isArray(value.files) ? value.files.slice(0, 64) : []).flatMap((item) =>
      isRecord(item) && typeof item.path === 'string'
        ? [
            {
              path: text(item.path, 512),
              added: integer(item.added),
              removed: integer(item.removed),
            },
          ]
        : [],
    ),
    deleted: (Array.isArray(value.deleted) ? value.deleted.slice(0, 64) : []).filter(
      (path): path is string => typeof path === 'string',
    ),
  }
}

export function parseComponentVersions(value: unknown): ComponentVersions {
  const current = isRecord(value) ? parseVersion(value.current) : null
  if (!current || !isRecord(value) || !Array.isArray(value.versions))
    throw new Error('Invalid custom UI versions response')
  return {
    current,
    versions: value.versions.flatMap((item) => parseVersion(item) ?? []),
    max: integer(value.max) || 20,
  }
}

export function listComponentVersions(id: string, signal?: AbortSignal) {
  return requestJson(`${componentPath(id)}/versions`, { signal, parse: parseComponentVersions })
}

export type ComponentDiff = {
  diff: string
  truncated: boolean
  files: { path: string; status: 'added' | 'deleted' | 'modified' }[]
}

export function parseComponentDiff(value: unknown): ComponentDiff | null {
  if (!isRecord(value) || typeof value.diff !== 'string') return null
  return {
    diff: value.diff,
    truncated: value.truncated === true,
    files: (Array.isArray(value.files) ? value.files.slice(0, 256) : []).flatMap((item) =>
      isRecord(item) &&
      typeof item.path === 'string' &&
      (item.status === 'added' || item.status === 'deleted' || item.status === 'modified')
        ? [{ path: item.path, status: item.status }]
        : [],
    ),
  }
}

export function diffComponentVersion(id: string, n: number, signal?: AbortSignal) {
  return requestJson(`${componentPath(id)}/versions/${n}/diff`, {
    signal,
    parse: (value) => {
      const diff = parseComponentDiff(value)
      if (!diff) throw new Error('Invalid custom UI diff response')
      return diff
    },
  })
}

export function restoreComponentVersion(id: string, n: number) {
  return requestJson(`${componentPath(id)}/versions/${n}/restore`, {
    method: 'POST',
    body: {},
    parse: (value) => ({ ...parseComponentVersions(value), component: rowResponse(value) }),
  })
}

// —— 导出 ——

export function exportComponentZip(id: string, signal?: AbortSignal) {
  return requestBlob(`${componentPath(id)}/export`, { signal, timeout: 60_000 })
}

// —— 回收站 ——

export type TrashEntry = {
  entry: string
  id: string
  name: string
  version: string
  kind: 'native' | 'legacy' | 'broken'
  reason: 'deleted' | 'legacy-retired'
  deletedAt: string
  // 旧组件迁移进来的记录不自动清理：null。
  expiresAt: string | null
  instances: number
}

export function parseTrash(value: unknown): { retentionDays: number; entries: TrashEntry[] } {
  if (!isRecord(value) || !Array.isArray(value.entries))
    throw new Error('Invalid custom UI trash response')
  return {
    retentionDays: integer(value.retentionDays) || 7,
    entries: value.entries.slice(0, 200).flatMap((item) =>
      isRecord(item) &&
      typeof item.entry === 'string' &&
      /^[a-z0-9][a-z0-9._-]{0,63}-\d{8}T\d{6}Z(?:-\d{1,4})?$/.test(item.entry) &&
      typeof item.id === 'string'
        ? [
            {
              entry: item.entry,
              id: text(item.id, 64),
              name: text(item.name, 120) || text(item.id, 64),
              version: text(item.version, 64),
              kind:
                item.kind === 'native' || item.kind === 'legacy' ? item.kind : ('broken' as const),
              reason: item.reason === 'legacy-retired' ? 'legacy-retired' : ('deleted' as const),
              deletedAt: text(item.deletedAt, 64),
              expiresAt: typeof item.expiresAt === 'string' ? item.expiresAt : null,
              instances: integer(item.instances),
            },
          ]
        : [],
    ),
  }
}

export function listComponentTrash(signal?: AbortSignal) {
  return requestJson('/api/custom-ui/trash', { signal, parse: parseTrash })
}

export function restoreTrashEntry(entry: string) {
  return requestJson(`/api/custom-ui/trash/${encodeURIComponent(entry)}/restore`, {
    method: 'POST',
    body: {},
    parse: (value) => ({
      component: rowResponse(value),
      restoredInstances: isRecord(value) ? integer(value.restoredInstances) : 0,
    }),
  })
}

export function purgeTrashEntry(entry: string) {
  return requestJson(`/api/custom-ui/trash/${encodeURIComponent(entry)}`, {
    method: 'DELETE',
    parse: () => undefined,
  })
}

// —— 导入暂存与信任确认（规格 5.7） ——

export const IMPORT_MAX_BYTES = 16 * 1024 * 1024

export function stageComponentImport(file: File) {
  if (file.size > IMPORT_MAX_BYTES)
    throw new ApiError('Component archive exceeds 16 MiB', {
      kind: 'protocol',
      data: { code: 'component_archive_too_large' },
    })
  const name = encodeURIComponent(file.name.slice(0, 200))
  return requestJson(`/api/custom-ui/import?stage=1&name=${name}`, {
    method: 'POST',
    data: file,
    timeout: 120_000,
    parse: (value) => {
      const staged = isRecord(value) ? parseStagedComponent(value.staged) : null
      if (!staged) throw new Error('Invalid custom UI staged response')
      return staged
    },
  })
}

export type StagedDetail = {
  staged: StagedComponent
  manifest: NativeManifest | null
  errors: ComponentFieldError[]
  files: ComponentFileEntry[]
  scan: ComponentScan
  installed: ComponentRow | null
  diff: ComponentDiff | null
}

export function parseStagedDetail(value: unknown): StagedDetail {
  const staged = isRecord(value) ? parseStagedComponent(value.staged) : null
  if (!staged || !isRecord(value)) throw new Error('Invalid custom UI staged response')
  return {
    staged,
    manifest: parseNativeManifest(staged.id, value.manifest),
    errors: parseFieldErrors(value.errors),
    files: parseComponentFiles(value.files),
    scan: parseComponentScan(value.scan),
    installed: value.installed ? parseComponentRow(value.installed) : null,
    diff: value.diff ? parseComponentDiff(value.diff) : null,
  }
}

const stagedPath = (stageId: string) => `/api/custom-ui/staged/${encodeURIComponent(stageId)}`

export function getStagedComponent(stageId: string, signal?: AbortSignal) {
  return requestJson(stagedPath(stageId), { signal, parse: parseStagedDetail })
}

export type StagedFile = {
  path: string
  binary: boolean
  content: string
  truncated: boolean
}

export function readStagedFile(stageId: string, path: string, signal?: AbortSignal) {
  return requestJson(`${stagedPath(stageId)}/file?path=${encodeURIComponent(path)}`, {
    signal,
    parse: (value): StagedFile => {
      if (!isRecord(value) || typeof value.content !== 'string')
        throw new Error('Invalid custom UI staged file response')
      return {
        path: text(value.path, 512),
        binary: value.binary === true,
        content: value.content,
        truncated: value.truncated === true,
      }
    },
  })
}

export function trustStagedComponent(stageId: string) {
  return requestJson(`${stagedPath(stageId)}/trust`, {
    method: 'POST',
    body: {},
    parse: (value) => ({
      component: rowResponse(value),
      replaced: isRecord(value) && value.replaced === true,
    }),
  })
}

export function discardStagedComponent(stageId: string) {
  return requestJson(`${stagedPath(stageId)}/discard`, {
    method: 'POST',
    body: {},
    parse: () => undefined,
  })
}

// —— 能力台账与撤销（规格 5.4） ——

export type CapabilityUsage = { lastAt: string; target: string; today: number; total: number }
export type ComponentUsage = {
  usage: Partial<Record<CapabilityType, CapabilityUsage>>
  revoked: CapabilityType[]
}

export function parseComponentUsage(value: unknown): ComponentUsage {
  if (!isRecord(value) || !isRecord(value.usage))
    throw new Error('Invalid custom UI usage response')
  const usage: ComponentUsage['usage'] = {}
  for (const type of CAPABILITIES) {
    const entry = value.usage[type]
    if (!isRecord(entry) || typeof entry.lastAt !== 'string') continue
    usage[type] = {
      lastAt: entry.lastAt,
      target: text(entry.target, 120),
      today: integer(entry.today),
      total: integer(entry.total),
    }
  }
  const revoked = Array.isArray(value.revoked) ? value.revoked : []
  return { usage, revoked: CAPABILITIES.filter((type) => revoked.includes(type)) }
}

export function getComponentUsage(id: string, signal?: AbortSignal) {
  return requestJson(`${componentPath(id)}/usage`, { signal, parse: parseComponentUsage })
}

export function setRevokedCapabilities(id: string, revoked: CapabilityType[]) {
  return requestJson(componentPath(id), {
    method: 'PATCH',
    body: { revoked },
    parse: rowResponse,
  })
}
