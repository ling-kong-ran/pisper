import { defineTool } from '@earendil-works/pi-coding-agent'
import { Type } from 'typebox'

export const manifest = {
  id: 'generate_visual',
  name: 'Visual Generate',
  category: '视觉',
  risk: '高风险',
  description: '自动调用已配置的视觉 Provider 生成图片、编辑图片或生成视频，并保存到工作目录。',
  scope: '已配置的视觉 Provider；当前会话工作目录/generated/visuals',
  capability: '自动汇总已启用视觉模型，调用生成图片、编辑图片或生成视频接口，并写入输出文件',
  source: 'app',
}

const optionalStringEnum = (values) => Type.Optional(Type.Union(values.map((value) => Type.Literal(value))))

export function createVisualGenerateTool({ cwd, visualGenerationService, onGeneratedFile }) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: manifest.description,
    promptSnippet: 'Generate or edit an image, or generate a video, with configured visual models',
    promptGuidelines: [
      'Use generate_visual when the user asks to create an image, illustration, poster, concept art, animation, or video.',
      'When the user asks to modify an existing image, pass its local path in sourceImages. The tool will automatically use an image editing interface.',
      'Before using generate_visual, include all important subject, style, composition, lighting, camera, motion, and text requirements in its prompt.',
      'generate_visual consumes external provider quota and writes the result under generated/visuals.',
      'Never claim that visual generation or editing succeeded unless generate_visual returned successfully; if it fails, report the tool error clearly.',
    ],
    parameters: Type.Object({
      kind: Type.Union([Type.Literal('image'), Type.Literal('video')], { description: '生成图片或视频' }),
      prompt: Type.String({ minLength: 1, description: '完整的视觉生成提示词' }),
      model: Type.Optional(Type.String({ description: '可选模型 ID，支持 provider/model；留空自动选择' })),
      sourceImages: Type.Optional(Type.Array(Type.String({ minLength: 1 }), { maxItems: 8, description: '需要编辑的本地图片路径；传入后自动调用图片编辑接口' })),
      maskPath: Type.Optional(Type.String({ minLength: 1, description: '可选 PNG 蒙版路径，仅用于图片编辑' })),
      outputName: Type.Optional(Type.String({ description: '输出文件名，不需要扩展名' })),
      aspectRatio: optionalStringEnum(['1:1', '16:9', '9:16', '4:3', '3:4']),
      size: optionalStringEnum(['1024x1024', '1536x1024', '1024x1536', '1280x720', '720x1280', '1792x1024', '1024x1792']),
      imageSize: optionalStringEnum(['1K', '2K', '4K']),
      resolution: optionalStringEnum(['720p', '1080p', '4k']),
      durationSeconds: Type.Optional(Type.Union([Type.Literal(4), Type.Literal(8), Type.Literal(12)])),
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
