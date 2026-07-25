import { defineTool } from '@earendil-works/pi-coding-agent'
import { Type } from 'typebox'

export const manifest = {
  id: 'generate_visual',
  name: 'Visual Generate',
  category: '视觉',
  risk: '高风险',
  description: '生成或编辑图片、设计图、效果图、插画、海报、Logo，或生成视频；调用已配置的视觉 Provider 并返回保存文件。',
  scope: '已配置的视觉 Provider；当前会话工作目录/generated/visuals',
  capability: '自动汇总已启用视觉模型，调用生成图片、编辑图片或生成视频接口，并写入输出文件',
  source: 'app',
}

const optionalStringEnum = (values) => Type.Optional(Type.String({ enum: values }))

export function createVisualGenerateTool({ cwd, visualGenerationService, onGeneratedFile }) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: manifest.description,
    promptSnippet: 'Generate or edit visual media with configured visual models',
    promptGuidelines: [
      'Use for image, mockup, concept art, poster, logo, animation, or video requests; call it instead of describing the visual.',
      'For edits, pass local paths in sourceImages. Include subject, style, composition, lighting, camera, motion, and text in prompt.',
      'Claim success only after a file path is returned; otherwise report the tool error.',
    ],
    parameters: Type.Object({
      kind: Type.String({ enum: ['image', 'video'], description: '生成图片或视频' }),
      prompt: Type.String({ minLength: 1, description: '完整的视觉生成提示词' }),
      model: Type.Optional(Type.String({ description: '可选模型 ID，支持 provider/model；留空自动选择' })),
      sourceImages: Type.Optional(Type.Array(Type.String({ minLength: 1 }), { maxItems: 8, description: '需要编辑的本地图片路径；传入后自动调用图片编辑接口' })),
      maskPath: Type.Optional(Type.String({ minLength: 1, description: '可选 PNG 蒙版路径，仅用于图片编辑' })),
      outputName: Type.Optional(Type.String({ description: '输出文件名，不需要扩展名' })),
      aspectRatio: optionalStringEnum(['1:1', '16:9', '9:16', '4:3', '3:4']),
      size: optionalStringEnum(['1024x1024', '1536x1024', '1024x1536', '1280x720', '720x1280', '1792x1024', '1024x1792']),
      imageSize: optionalStringEnum(['1K', '2K', '4K']),
      resolution: optionalStringEnum(['720p', '1080p', '4k']),
      durationSeconds: Type.Optional(Type.Number({ enum: [4, 8, 12] })),
      quality: optionalStringEnum(['auto', 'low', 'medium', 'high', 'standard', 'hd']),
      outputFormat: optionalStringEnum(['png', 'jpeg', 'webp']),
    }),
    async execute(_toolCallId, params, signal, onUpdate) {
      if (!visualGenerationService) throw new Error('视觉生成服务尚未初始化。')
      const result = await visualGenerationService.generate({ ...params, cwd }, {
        signal,
        onProgress: (message) => onUpdate?.({ content: [{ type: 'text', text: message }] }),
      })
      try {
        await onGeneratedFile?.(result)
      } catch {
        // Asset indexing must not discard a successfully generated file.
      }
      return {
        content: [{
          type: 'text',
          text: `${result.operation === 'edit' ? '图片已编辑' : result.kind === 'video' ? '视频已生成' : '图片已生成'}。\n文件：${result.path}\nProvider：${result.providerName}\n模型：${result.modelName}`,
        }],
        details: result,
      }
    },
  })
}
