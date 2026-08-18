// 视觉生成服务：图像/视频生成的总入口——模型选择、驱动分发、失败回退、
// 结果保存与资产登记；按 provider 协议分派到各驱动实现。
import { runVisualDriver } from './driver-registry.mjs'
import { VisualModelCatalog } from './model-selection.mjs'
import { saveVisualOutput } from './output.mjs'
import { loadMaskImage, loadSourceImages } from './source-images.mjs'

export { inferModelKind } from './model-selection.mjs'

const RETRYABLE_VISUAL_STATUSES = new Set([404, 408, 409, 425, 429, 500, 502, 503, 504])
const UNAVAILABLE_VISUAL_PATTERN =
  /(?:model[_ -]?not[_ -]?found|unknown provider for model|unsupported model|model .* unavailable|no available (?:channel|provider|model)|(?:token|key|credential).*(?:cannot|can't|not authorized to|no permission to|does not have).*access.*model|无可用渠道|模型不存在|模型不可用|未找到.*模型|没有.*渠道|(?:令牌|密钥|凭证).*无权访问模型)/i
const SAFETY_REJECTION_PATTERN =
  /(?:content policy|safety system|moderation|内容安全|安全策略|审核拒绝|违规内容)/i

// 可回退的错误判定：瞬时错误/模型不可用可回退到备选模型，安全拒绝不可回退。
function canFallbackFrom(error, signal) {
  if (signal?.aborted) return false
  const message = String(error?.message || error || '')
  if (SAFETY_REJECTION_PATTERN.test(message)) return false
  const status = Number(error?.status ?? error?.statusCode)
  return RETRYABLE_VISUAL_STATUSES.has(status) || UNAVAILABLE_VISUAL_PATTERN.test(message)
}

function noModelsError(kind) {
  return new Error(
    `没有已配置并启用的${kind === 'video' ? '视频' : '图像'}生成模型。请先在配置页添加视觉模型。`,
  )
}

function normalizeRequest(model, request) {
  const value = { ...request }
  if (!value.size && value.aspectRatio) {
    if (value.kind === 'video') {
      value.size = value.aspectRatio === '9:16' ? '720x1280' : '1280x720'
    } else if (!model.driver.startsWith('google-')) {
      value.size =
        value.aspectRatio === '9:16' || value.aspectRatio === '3:4'
          ? '1024x1536'
          : value.aspectRatio === '16:9' || value.aspectRatio === '4:3'
            ? '1536x1024'
            : '1024x1024'
    }
  }
  if (
    model.driver.startsWith('google-') &&
    value.kind === 'video' &&
    !value.resolution &&
    value.size
  ) {
    value.resolution = value.size.includes('1080') ? '1080p' : '720p'
  }
  return value
}

export class VisualGenerationService {
  constructor(paths) {
    this.models = new VisualModelCatalog(paths)
  }

  async generate(request, options = {}) {
    const kind = request.kind === 'video' ? 'video' : 'image'
    if (kind === 'video' && (request.sourceImages?.length || request.maskPath))
      throw new Error('视频生成暂不支持图片编辑参数。')
    const requestedModel = String(request.model || '').trim()
    const models = requestedModel
      ? [await this.models.select(kind, requestedModel)]
      : await this.models.list(kind)
    if (!models.length) throw noModelsError(kind)
    const sourceImages =
      kind === 'image' ? await loadSourceImages(request.sourceImages, request.cwd) : []
    const maskImage = kind === 'image' ? await loadMaskImage(request.maskPath, request.cwd) : null
    const operation = sourceImages.length ? 'edit' : 'generate'
    const attemptedModels = []

    for (const [index, model] of models.entries()) {
      attemptedModels.push(`${model.providerId}/${model.id}`)
      const normalizedRequest = normalizeRequest(model, {
        ...request,
        kind,
        operation,
        sourceImages,
        maskImage,
      })
      options.onProgress?.(
        `使用 ${model.providerName} / ${model.name} ${operation === 'edit' ? '编辑图片' : `生成${kind === 'video' ? '视频' : '图片'}`}…`,
      )
      try {
        const result = await runVisualDriver(model, normalizedRequest, options)
        const path = await saveVisualOutput({
          cwd: request.cwd,
          prompt: request.prompt,
          outputName: request.outputName,
          result,
        })
        return {
          path,
          kind,
          mimeType: result.mimeType,
          size: result.buffer.length,
          provider: model.providerId,
          providerName: model.providerName,
          model: model.id,
          modelName: model.name,
          remoteId: result.remoteId || null,
          operation,
          fallbackUsed: index > 0,
          attemptedModels,
        }
      } catch (error) {
        const nextModel = models[index + 1]
        if (requestedModel || !nextModel || !canFallbackFrom(error, options.signal)) throw error
        options.onProgress?.(
          `${model.providerName} / ${model.name} 当前不可用，尝试 ${nextModel.providerName} / ${nextModel.name}…`,
        )
      }
    }
    throw noModelsError(kind)
  }
}
