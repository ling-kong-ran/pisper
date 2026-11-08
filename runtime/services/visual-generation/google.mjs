// Google 视觉驱动：调用 Gemini 图像/视频生成接口（REST + 长任务轮询）。
import { pisperError } from '../../../shared/errors.mjs'
function googleHeaders(model) {
  return { 'Content-Type': 'application/json', 'x-goog-api-key': model.apiKey, ...model.headers }
}

async function googleJson(url, model, init, signal) {
  const response = await fetch(url, {
    ...init,
    headers: { ...googleHeaders(model), ...(init?.headers || {}) },
    signal,
  })
  const data = await response.json().catch(() => ({}))
  if (!response.ok)
    throw pisperError('failed', 'image', data.error?.message || `Google HTTP ${response.status}`)
  return data
}

function operationUrl(baseUrl, name) {
  const root = baseUrl.replace(/\/+$/, '')
  const cleanName = String(name || '').replace(/^\/+/, '')
  return `${root}/${cleanName}`
}

function findVideoUri(value) {
  if (!value || typeof value !== 'object') return ''
  if (typeof value.uri === 'string' && /video|files|download|googleapis/i.test(value.uri))
    return value.uri
  for (const child of Array.isArray(value) ? value : Object.values(value)) {
    const found = findVideoUri(child)
    if (found) return found
  }
  return ''
}

async function generateGoogleImage(model, request, signal) {
  const modelId = model.id.replace(/^models\//, '')
  const sourceParts = (request.sourceImages || []).map((image) => ({
    inlineData: { mimeType: image.mimeType, data: image.buffer.toString('base64') },
  }))
  const imageConfig = {
    ...(request.aspectRatio ? { aspectRatio: request.aspectRatio } : {}),
    ...(request.imageSize ? { imageSize: request.imageSize } : {}),
  }
  const data = await googleJson(
    `${model.baseUrl}/models/${encodeURIComponent(modelId)}:generateContent`,
    model,
    {
      method: 'POST',
      body: JSON.stringify({
        contents: [{ role: 'user', parts: [...sourceParts, { text: request.prompt }] }],
        generationConfig: {
          responseModalities: ['TEXT', 'IMAGE'],
          ...(Object.keys(imageConfig).length ? { imageConfig } : {}),
        },
      }),
    },
    signal,
  )
  const parts = data.candidates?.flatMap((candidate) => candidate.content?.parts || []) || []
  const inline = parts.map((part) => part.inlineData || part.inline_data).find((part) => part?.data)
  if (!inline) throw pisperError('failed', 'image', 'Gemini returned no image data')
  const mimeType = inline.mimeType || inline.mime_type || 'image/png'
  return {
    buffer: Buffer.from(inline.data, 'base64'),
    mimeType,
    extension: mimeType.includes('jpeg') ? '.jpg' : mimeType.includes('webp') ? '.webp' : '.png',
  }
}

async function generateGoogleVideo(model, request, signal, onProgress) {
  const modelId = model.id.replace(/^models\//, '')
  let operation = await googleJson(
    `${model.baseUrl}/models/${encodeURIComponent(modelId)}:predictLongRunning`,
    model,
    {
      method: 'POST',
      body: JSON.stringify({
        instances: [{ prompt: request.prompt }],
        parameters: {
          sampleCount: 1,
          ...(request.aspectRatio ? { aspectRatio: request.aspectRatio } : {}),
          ...(request.durationSeconds ? { durationSeconds: request.durationSeconds } : {}),
          ...(request.resolution ? { resolution: request.resolution } : {}),
        },
      }),
    },
    signal,
  )
  if (!operation.name)
    throw pisperError('failed', 'image', 'Google video API returned no operation ID')
  while (!operation.done) {
    onProgress?.('Generating video; waiting for Google Veo…')
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
    operation = await googleJson(
      operationUrl(model.baseUrl, operation.name),
      model,
      { method: 'GET' },
      signal,
    )
  }
  if (operation.error)
    throw pisperError('failed', 'image', operation.error.message || 'Google video generation')
  const uri = findVideoUri(operation.response)
  if (!uri)
    throw pisperError('failed', 'image', 'Google video finished without a downloadable file')
  onProgress?.('Video ready; downloading…')
  const response = await fetch(uri, { headers: { 'x-goog-api-key': model.apiKey }, signal })
  if (!response.ok)
    throw pisperError('failed', 'image', `Google video download: HTTP ${response.status}`)
  return {
    buffer: Buffer.from(await response.arrayBuffer()),
    mimeType: response.headers.get('content-type') || 'video/mp4',
    extension: '.mp4',
    remoteId: operation.name,
  }
}

export function generateGoogle(model, request, options = {}) {
  return request.kind === 'video'
    ? generateGoogleVideo(model, request, options.signal, options.onProgress)
    : generateGoogleImage(model, request, options.signal)
}
