// xAI 视觉驱动：调用 Grok 图像/视频生成接口。
import { pisperError } from '../../../shared/errors.mjs'
function authHeaders(model, extra = {}) {
  return {
    Authorization: `Bearer ${model.apiKey}`,
    ...model.headers,
    ...extra,
  }
}

async function jsonRequest(url, model, init, signal) {
  const response = await fetch(url, {
    ...init,
    headers: authHeaders(model, { 'Content-Type': 'application/json', ...(init.headers || {}) }),
    signal,
  })
  const data = await response.json().catch(() => ({}))
  if (!response.ok) {
    const message =
      data.error?.message || data.error || data.message || `xAI HTTP ${response.status}`
    const error = pisperError('failed', 'provider', String(message), { status: response.status })
    error.status = response.status
    throw error
  }
  return data
}

async function download(url, model, signal) {
  const response = await fetch(url, { headers: authHeaders(model), signal })
  if (!response.ok) throw pisperError('failed', 'image', `xAI download: HTTP ${response.status}`)
  return {
    buffer: Buffer.from(await response.arrayBuffer()),
    mimeType: response.headers.get('content-type') || 'application/octet-stream',
  }
}

function extensionFor(mimeType, fallback = '.png') {
  if (mimeType.includes('jpeg')) return '.jpg'
  if (mimeType.includes('webp')) return '.webp'
  if (mimeType.includes('mp4')) return '.mp4'
  if (mimeType.includes('webm')) return '.webm'
  return fallback
}

function imageResult(data) {
  const value = data.data?.[0] || data.image || data.output?.[0] || data.output || {}
  const base64 = value.b64_json || value.base64 || value.image_base64
  if (base64) {
    const mimeType = value.mime_type || value.mimeType || 'image/png'
    return { buffer: Buffer.from(base64, 'base64'), mimeType, extension: extensionFor(mimeType) }
  }
  return { url: value.url || value.image_url || data.url || data.image_url || '' }
}

async function generateImage(model, request, signal) {
  const root = model.baseUrl.replace(/\/+$/, '')
  if (request.operation === 'edit') {
    const form = new FormData()
    form.set('model', model.id)
    form.set('prompt', request.prompt)
    request.sourceImages.forEach((image, index) =>
      form.append(
        'image',
        new Blob([image.buffer], { type: image.mimeType }),
        `image-${index + 1}`,
      ),
    )
    if (request.maskImage)
      form.set(
        'mask',
        new Blob([request.maskImage.buffer], { type: request.maskImage.mimeType }),
        'mask.png',
      )
    if (request.size) form.set('size', request.size)
    if (request.quality) form.set('quality', request.quality)
    const response = await fetch(`${root}/images/edits`, {
      method: 'POST',
      headers: authHeaders(model),
      body: form,
      signal,
    })
    const data = await response.json().catch(() => ({}))
    if (!response.ok)
      throw pisperError(
        'failed',
        'image',
        String(
          data.error?.message || data.error || data.message || `xAI edit: HTTP ${response.status}`,
        ),
      )
    const result = imageResult(data)
    if (result.buffer) return result
    if (result.url) {
      const downloaded = await download(result.url, model, signal)
      return { ...downloaded, extension: extensionFor(downloaded.mimeType) }
    }
    throw pisperError('failed', 'image', 'xAI edit returned no image data')
  }

  const data = await jsonRequest(
    `${root}/images/generations`,
    model,
    {
      method: 'POST',
      body: JSON.stringify({
        model: model.id,
        prompt: request.prompt,
        n: 1,
        response_format: 'b64_json',
        ...(request.aspectRatio ? { aspect_ratio: request.aspectRatio } : {}),
        ...(request.outputFormat ? { output_format: request.outputFormat } : {}),
      }),
    },
    signal,
  )
  const result = imageResult(data)
  if (result.buffer) return result
  if (result.url) {
    const downloaded = await download(result.url, model, signal)
    return { ...downloaded, extension: extensionFor(downloaded.mimeType) }
  }
  throw pisperError('failed', 'image', 'xAI returned no image data')
}

function videoUrl(value) {
  if (!value || typeof value !== 'object') return ''
  for (const key of ['url', 'video_url', 'download_url']) {
    if (typeof value[key] === 'string' && /^https?:/i.test(value[key])) return value[key]
  }
  for (const child of Array.isArray(value) ? value : Object.values(value)) {
    const found = videoUrl(child)
    if (found) return found
  }
  return ''
}

function videoStatus(value) {
  return String(value?.status || value?.state || value?.data?.status || '').toLowerCase()
}

async function createVideo(root, model, request, signal) {
  const size = String(request.size || (request.aspectRatio === '9:16' ? '720x1280' : '1280x720'))
  const xaiBody = {
    model: model.id,
    prompt: request.prompt,
    ...(request.durationSeconds ? { duration: request.durationSeconds } : {}),
    ...(request.aspectRatio ? { aspect_ratio: request.aspectRatio } : {}),
    ...(request.resolution ? { resolution: request.resolution } : {}),
  }
  try {
    const task = await jsonRequest(
      `${root}/videos/generations`,
      model,
      { method: 'POST', body: JSON.stringify(xaiBody) },
      signal,
    )
    return { ...task, _pisperEndpoint: 'videos/generations' }
  } catch (error) {
    if (![404, 405].includes(error.status)) throw error
  }
  const task = await jsonRequest(
    `${root}/videos`,
    model,
    {
      method: 'POST',
      body: JSON.stringify({
        model: model.id,
        prompt: request.prompt,
        seconds: request.durationSeconds ? String(request.durationSeconds) : undefined,
        size,
      }),
    },
    signal,
  )
  return { ...task, _pisperEndpoint: 'videos' }
}

async function getVideo(root, id, endpoint, model, signal) {
  if (endpoint === 'videos/generations') {
    return jsonRequest(
      `${root}/videos/generations/${encodeURIComponent(id)}`,
      model,
      { method: 'GET' },
      signal,
    )
  }
  try {
    return await jsonRequest(
      `${root}/videos/${encodeURIComponent(id)}`,
      model,
      { method: 'GET' },
      signal,
    )
  } catch (error) {
    if (![404, 405].includes(error.status)) throw error
    return jsonRequest(
      `${root}/videos/generations/${encodeURIComponent(id)}`,
      model,
      { method: 'GET' },
      signal,
    )
  }
}

async function generateVideo(model, request, signal, onProgress) {
  const root = model.baseUrl.replace(/\/+$/, '')
  let task = await createVideo(root, model, request, signal)
  const id = task.task_id || task.request_id || task.id || task.data?.task_id || task.data?.id
  const endpoint = task._pisperEndpoint
  let url = videoUrl(task)
  while (!url && id) {
    const status = videoStatus(task)
    if (['failed', 'error', 'cancelled', 'canceled'].includes(status))
      throw pisperError(
        'failed',
        'image',
        task.error?.message || task.error || 'xAI video generation',
      )
    if (['completed', 'succeeded', 'done'].includes(status)) break
    onProgress?.(`Generating video${status ? `: ${status}` : ''}…`)
    await new Promise((resolve, reject) => {
      const timer = setTimeout(resolve, 5000)
      signal?.addEventListener(
        'abort',
        () => {
          clearTimeout(timer)
          reject(signal.reason || pisperError('cancelled', 'operation'))
        },
        { once: true },
      )
    })
    task = await getVideo(root, id, endpoint, model, signal)
    url = videoUrl(task)
  }
  if (url) {
    const downloaded = await download(url, model, signal)
    return {
      ...downloaded,
      extension: extensionFor(downloaded.mimeType, '.mp4'),
      remoteId: id || null,
    }
  }
  if (!id) throw pisperError('failed', 'image', 'xAI video returned no task ID or URL')
  const response = await fetch(`${root}/videos/${encodeURIComponent(id)}/content`, {
    headers: authHeaders(model),
    signal,
  })
  if (!response.ok)
    throw pisperError('failed', 'image', `xAI video download: HTTP ${response.status}`)
  const mimeType = response.headers.get('content-type') || 'video/mp4'
  return {
    buffer: Buffer.from(await response.arrayBuffer()),
    mimeType,
    extension: extensionFor(mimeType, '.mp4'),
    remoteId: id,
  }
}

export function generateXAI(model, request, options = {}) {
  return request.kind === 'video'
    ? generateVideo(model, request, options.signal, options.onProgress)
    : generateImage(model, request, options.signal)
}
