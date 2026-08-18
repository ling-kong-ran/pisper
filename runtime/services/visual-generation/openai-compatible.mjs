// OpenAI 兼容视觉驱动：适用于 OpenAI/OpenRouter 等 OpenAI 协议兼容的图像/视频生成。
import { basename } from 'node:path'
import OpenAI, { toFile } from 'openai'

function dataUrlImage(value) {
  const match = String(value || '').match(/^data:([^;]+);base64,(.+)$/)
  return match ? { mimeType: match[1], buffer: Buffer.from(match[2], 'base64') } : null
}

async function download(url, signal, headers = {}) {
  const response = await fetch(url, { signal, headers })
  if (!response.ok) throw new Error(`下载生成结果失败 (${response.status})`)
  return {
    buffer: Buffer.from(await response.arrayBuffer()),
    mimeType: response.headers.get('content-type') || 'application/octet-stream',
  }
}

function imageExtension(mimeType) {
  if (mimeType.includes('jpeg')) return '.jpg'
  if (mimeType.includes('webp')) return '.webp'
  return '.png'
}

async function openRouterImage(client, model, request, signal) {
  const imageContent = (request.sourceImages || []).map((image) => ({
    type: 'image_url',
    image_url: { url: `data:${image.mimeType};base64,${image.buffer.toString('base64')}` },
  }))
  const response = await client.chat.completions.create(
    {
      model: model.id,
      messages: [
        { role: 'user', content: [{ type: 'text', text: request.prompt }, ...imageContent] },
      ],
      modalities: ['image'],
      stream: false,
    },
    { signal },
  )
  const images = response.choices?.[0]?.message?.images || []
  const imageUrl =
    typeof images[0]?.image_url === 'string' ? images[0].image_url : images[0]?.image_url?.url
  const inline = dataUrlImage(imageUrl)
  if (inline) return { ...inline, extension: imageExtension(inline.mimeType) }
  if (imageUrl) {
    const value = await download(imageUrl, signal)
    return { ...value, extension: imageExtension(value.mimeType) }
  }
  throw new Error('视觉模型没有返回图片数据。')
}

async function openAIImage(client, model, request, signal) {
  const isOpenAIImage = /gpt.*image|dall-e/i.test(model.id)
  const common = {
    model: model.id,
    prompt: request.prompt,
    n: 1,
    ...(request.size ? { size: request.size } : {}),
    ...(request.quality ? { quality: request.quality } : {}),
    ...(isOpenAIImage ? { output_format: request.outputFormat || 'png' } : {}),
  }
  let response
  if (request.operation === 'edit') {
    const images = await Promise.all(
      request.sourceImages.map((image) =>
        toFile(image.buffer, basename(image.path), { type: image.mimeType }),
      ),
    )
    const mask = request.maskImage
      ? await toFile(request.maskImage.buffer, basename(request.maskImage.path), {
          type: request.maskImage.mimeType,
        })
      : undefined
    response = await client.images.edit(
      {
        ...common,
        image: images.length === 1 ? images[0] : images,
        ...(mask ? { mask } : {}),
      },
      { signal },
    )
  } else {
    response = await client.images.generate(common, { signal })
  }
  const image = response.data?.[0]
  if (image?.b64_json) {
    const mimeType =
      request.outputFormat === 'jpeg'
        ? 'image/jpeg'
        : request.outputFormat === 'webp'
          ? 'image/webp'
          : 'image/png'
    return {
      buffer: Buffer.from(image.b64_json, 'base64'),
      mimeType,
      extension: imageExtension(mimeType),
    }
  }
  if (image?.url) {
    const value = await download(image.url, signal)
    return { ...value, extension: imageExtension(value.mimeType) }
  }
  throw new Error('视觉模型没有返回图片数据。')
}

async function openAIVideo(client, model, request, signal, onProgress) {
  let video = await client.videos.create(
    {
      model: model.id,
      prompt: request.prompt,
      ...(request.durationSeconds ? { seconds: String(request.durationSeconds) } : {}),
      ...(request.size ? { size: request.size } : {}),
    },
    { signal },
  )
  while (!['completed', 'failed'].includes(video.status)) {
    onProgress?.(`视频生成中：${Math.round(video.progress || 0)}%`)
    await new Promise((resolve, reject) => {
      const timer = setTimeout(resolve, 5000)
      signal?.addEventListener(
        'abort',
        () => {
          clearTimeout(timer)
          reject(signal.reason || new Error('已取消'))
        },
        { once: true },
      )
    })
    video = await client.videos.retrieve(video.id, { signal })
  }
  if (video.status === 'failed') throw new Error(video.error?.message || '视频生成失败。')
  onProgress?.('视频已生成，正在下载…')
  const response = await client.videos.downloadContent(video.id, { variant: 'video' }, { signal })
  return {
    buffer: Buffer.from(await response.arrayBuffer()),
    mimeType: response.headers.get('content-type') || 'video/mp4',
    extension: '.mp4',
    remoteId: video.id,
  }
}

export async function generateOpenAICompatible(model, request, { signal, onProgress } = {}) {
  const client = new OpenAI({
    apiKey: model.apiKey,
    baseURL: model.baseUrl,
    defaultHeaders: model.headers,
    timeout: request.kind === 'video' ? 10 * 60_000 : 3 * 60_000,
    maxRetries: 1,
  })
  if (request.kind === 'video') return openAIVideo(client, model, request, signal, onProgress)
  if (model.driver === 'openrouter-image') return openRouterImage(client, model, request, signal)
  return openAIImage(client, model, request, signal)
}
