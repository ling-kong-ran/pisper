// New API 视觉驱动：适配 New API 中转站的图像/视频生成协议，
// 附带针对中转站常见转发错误的友好提示。
import { generateOpenAICompatible } from './openai-compatible.mjs'

function errorText(value) {
  if (typeof value === 'string') return value.trim()
  if (value && typeof value.message === 'string') return value.message.trim()
  if (value == null) return ''
  try {
    return JSON.stringify(value)
  } catch {
    return String(value)
  }
}

function newAPIRequestError(data, status) {
  const upstream = errorText(data?.error?.message || data?.error || data?.message)
  if (/duplicate field\s+[`'"]?duration/i.test(upstream)) {
    return new Error(
      `New API 视频渠道转发失败：${upstream}。请检查中转站中该模型的渠道映射或协议适配。`,
    )
  }
  return new Error(upstream || `New API 视觉接口请求失败 (${status})`)
}

function headers(model, extra = {}) {
  return {
    Authorization: `Bearer ${model.apiKey}`,
    ...model.headers,
    ...extra,
  }
}

async function jsonRequest(url, model, init, signal) {
  const response = await fetch(url, {
    ...init,
    headers: headers(model, { 'Content-Type': 'application/json', ...(init.headers || {}) }),
    signal,
  })
  const data = await response.json().catch(() => ({}))
  if (!response.ok) {
    const error = newAPIRequestError(data, response.status)
    error.status = response.status
    throw error
  }
  return data
}

function videoStatus(value) {
  return String(value?.status || value?.state || value?.data?.status || '').toLowerCase()
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

function taskId(value) {
  return (
    value?.task_id ||
    value?.request_id ||
    value?.id ||
    value?.data?.task_id ||
    value?.data?.id ||
    ''
  )
}

async function wait(delayMs, signal) {
  await new Promise((resolve, reject) => {
    const timer = setTimeout(resolve, delayMs)
    signal?.addEventListener(
      'abort',
      () => {
        clearTimeout(timer)
        reject(signal.reason || new Error('已取消'))
      },
      { once: true },
    )
  })
}

async function download(url, model, signal) {
  const response = await fetch(url, { headers: headers(model), signal })
  if (!response.ok) throw new Error(`下载 New API 视频失败 (${response.status})`)
  const mimeType = response.headers.get('content-type') || 'video/mp4'
  return {
    buffer: Buffer.from(await response.arrayBuffer()),
    mimeType,
    extension: mimeType.includes('webm') ? '.webm' : '.mp4',
  }
}

async function createOpenAIVideo(root, model, request, signal) {
  const size = String(request.size || (request.aspectRatio === '9:16' ? '720x1280' : '1280x720'))
  const task = await jsonRequest(
    `${root}/videos`,
    model,
    {
      method: 'POST',
      body: JSON.stringify({
        model: model.id,
        prompt: request.prompt,
        ...(request.durationSeconds ? { seconds: String(request.durationSeconds) } : {}),
        size,
      }),
    },
    signal,
  )
  return { task, route: 'videos' }
}

async function createLegacyVideo(root, model, request, signal) {
  const size = String(request.size || (request.aspectRatio === '9:16' ? '720x1280' : '1280x720'))
  const [width, height] = size.split('x').map(Number)
  const task = await jsonRequest(
    `${root}/video/generations`,
    model,
    {
      method: 'POST',
      body: JSON.stringify({
        model: model.id,
        prompt: request.prompt,
        ...(request.durationSeconds ? { duration: request.durationSeconds } : {}),
        ...(width && height ? { width, height } : {}),
        response_format: 'url',
      }),
    },
    signal,
  )
  return { task, route: 'video/generations' }
}

async function createVideo(root, model, request, signal) {
  try {
    return await createOpenAIVideo(root, model, request, signal)
  } catch (error) {
    if (![404, 405].includes(error.status)) throw error
    return createLegacyVideo(root, model, request, signal)
  }
}

async function generateVideo(model, request, signal, onProgress) {
  const root = model.baseUrl.replace(/\/+$/, '')
  let { task, route } = await createVideo(root, model, request, signal)
  const id = taskId(task)
  let url = videoUrl(task)
  while (!url && id) {
    const status = videoStatus(task)
    if (['failed', 'error', 'cancelled', 'canceled'].includes(status)) {
      throw newAPIRequestError({ error: task.error }, 502)
    }
    if (['completed', 'succeeded', 'done'].includes(status)) break
    onProgress?.(`视频生成中${status ? `：${status}` : ''}…`)
    await wait(5000, signal)
    task = await jsonRequest(
      `${root}/${route}/${encodeURIComponent(id)}`,
      model,
      { method: 'GET' },
      signal,
    )
    url = videoUrl(task)
  }
  if (url) return { ...(await download(url, model, signal)), remoteId: id || null }
  if (!id) throw new Error('New API 视频接口没有返回任务 ID。')
  return {
    ...(await download(`${root}/videos/${encodeURIComponent(id)}/content`, model, signal)),
    remoteId: id,
  }
}

export function generateNewAPI(model, request, options = {}) {
  if (request.kind !== 'video') return generateOpenAICompatible(model, request, options)
  return generateVideo(model, request, options.signal, options.onProgress)
}
