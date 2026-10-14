// @public 自定义组件的素材桥，仅访问独立素材项目和受控媒体；不暴露任意 HTTP 或文件路径。
import { requestJson, requestBlob } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'
import { parseWorkflowMedia } from '@shared/workflow-inputs.mjs'
import { normalizeImageSettings } from '@shared/image-operations.mjs'
import { normalizeImageFrameEdits } from '@shared/image-frame-edits.mjs'
import { parseSpriteEngineCatalog } from '@shared/sprite-engine-catalog.mjs'
import {
  parseGameAssetsCatalog,
  parseGameAssetProjectInput,
  parseGameAssetProject,
  parseGameAssetJob,
} from '@shared/game-assets.mjs'

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalidResponseError()
  return value as Record<string, unknown>
}
function id(value: unknown) {
  if (typeof value !== 'string' || !/^[0-9a-f-]{36}$/i.test(value)) throw invalidResponseError()
  return encodeURIComponent(value)
}
function download(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = name
  anchor.click()
  window.setTimeout(() => URL.revokeObjectURL(url), 1000)
}
function parseCatalog(value: unknown) {
  const input = record(value)
  const state = parseGameAssetsCatalog({ projects: input.projects, jobs: input.jobs })
  if (!Array.isArray(input.models) || input.models.length > 1000) throw invalidResponseError()
  const models = input.models.map((value: unknown) => {
    const model = record(value)
    if (
      typeof model.id !== 'string' ||
      typeof model.name !== 'string' ||
      typeof model.providerId !== 'string' ||
      typeof model.providerName !== 'string' ||
      !model.id.startsWith(model.providerId + '/')
    )
      throw invalidResponseError()
    return {
      id: model.id,
      name: model.name,
      providerId: model.providerId,
      providerName: model.providerName,
    }
  })
  return { ...state, models, engines: parseSpriteEngineCatalog({ engines: input.engines }).engines }
}
const mediaUrl = (value: unknown) => `/api/game-assets/media/${id(value)}/content`

export async function handleGameAssetComponentRequest(
  method: string,
  params: Record<string, unknown>,
  signal: AbortSignal,
): Promise<unknown> {
  signal.throwIfAborted()
  if (method === 'gameAssets.list')
    return requestJson('/api/game-assets', { signal, parse: parseCatalog })
  if (method === 'gameAssets.save') {
    const payload = parseGameAssetProjectInput(params)
    return requestJson(
      payload.id ? `/api/game-assets/projects/${id(payload.id)}` : '/api/game-assets/projects',
      {
        method: payload.id ? 'PATCH' : 'POST',
        data: payload,
        signal,
        parse: parseGameAssetProject,
      },
    )
  }
  if (method === 'gameAssets.remove')
    return requestJson(`/api/game-assets/projects/${id(params.id)}`, { method: 'DELETE', signal })
  if (method === 'gameAssets.run' || method === 'gameAssets.stop') {
    const endpoint =
      method === 'gameAssets.run'
        ? `/api/game-assets/projects/${id(params.id)}/run`
        : `/api/game-assets/jobs/${id(params.jobId)}/stop`
    return requestJson(endpoint, {
      method: 'POST',
      data: {},
      signal,
      parse: (value) => ({ job: parseGameAssetJob(record(value).job) }),
    })
  }
  if (method === 'gameAssets.editFrames')
    return requestJson(`/api/game-assets/jobs/${id(params.jobId)}/frames`, {
      method: 'POST',
      data: normalizeImageFrameEdits(params.edits),
      signal,
      timeout: 120000,
      parse: parseGameAssetJob,
    })
  if (method === 'gameAssets.export') {
    const job = await requestJson(`/api/game-assets/jobs/${id(params.jobId)}`, {
      signal,
      parse: parseGameAssetJob,
    })
    if (!job.output.atlas) throw invalidResponseError()
    if (params.format === 'png') {
      const blob = await requestBlob(mediaUrl(job.output.atlas.media.id), { signal })
      signal.throwIfAborted()
      download(blob, 'game-assets.png')
    } else if (params.format === 'json')
      download(
        new Blob([JSON.stringify(job.output.atlas, null, 2)], { type: 'application/json' }),
        'game-assets.json',
      )
    else throw invalidResponseError()
    return null
  }
  if (method === 'gameAssets.uploadImage') {
    if (
      !(params.buffer instanceof ArrayBuffer) ||
      !params.buffer.byteLength ||
      params.buffer.byteLength > 8 * 1024 * 1024 ||
      !['image/png', 'image/jpeg', 'image/webp'].includes(String(params.mimeType)) ||
      typeof params.name !== 'string' ||
      params.name.length > 200
    )
      throw invalidResponseError()
    return requestJson(`/api/game-assets/media?name=${encodeURIComponent(params.name)}`, {
      method: 'POST',
      data: new Blob([params.buffer], { type: String(params.mimeType) }),
      signal,
      parse: parseWorkflowMedia,
    })
  }
  if (method === 'gameAssets.image') {
    const blob = await requestBlob(mediaUrl(params.id), { signal })
    if (
      blob.size > 8 * 1024 * 1024 ||
      !['image/png', 'image/jpeg', 'image/webp'].includes(blob.type)
    )
      throw invalidResponseError()
    return { buffer: await blob.arrayBuffer(), mimeType: blob.type }
  }
  if (method === 'gameAssets.process') {
    if (!['background', 'inpaint'].includes(String(params.operation))) throw invalidResponseError()
    return requestJson('/api/game-assets/process', {
      method: 'POST',
      signal,
      timeout: 120000,
      data: {
        reference: parseWorkflowMedia(params.reference),
        operation: params.operation,
        image: normalizeImageSettings(params.image),
      },
      parse: parseWorkflowMedia,
    })
  }
  if (method === 'gameAssets.engine') {
    if (
      (params.id !== 'background' && params.id !== 'inpaint') ||
      (params.action !== 'download' && params.action !== 'cancel')
    )
      throw invalidResponseError()
    return requestJson(`/api/game-assets/engines/${params.id}/${params.action}`, {
      method: 'POST',
      data: {},
      signal,
      parse: parseSpriteEngineCatalog,
    })
  }
  throw invalidResponseError()
}
