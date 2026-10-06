// 视觉生成工具：图像/视频生成与编辑，产物写入工作区 generated/visuals。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'

export const manifest = {
  id: 'generate_visual',
  name: 'Visual Generate',
  category: 'visual',
  risk: 'high',
  description: 'Generate or edit images, mockups, posters, logos, or videos.',
  scope: 'Visual providers; workspace/generated/visuals',
  capability: 'Generate or edit images and videos',
  source: 'app',
}

const optionalStringEnum = (values) => Type.Optional(Type.String({ enum: values }))

export function createVisualGenerateTool({ cwd, visualGenerationService, onGeneratedFile }) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: manifest.description,
    promptSnippet: 'Generate or edit visual media',
    promptGuidelines: [
      'Call for image, mockup, poster, logo, animation, or video requests.',
      'For edits, pass local paths in sourceImages.',
      'Claim success only after a file path is returned.',
    ],
    parameters: Type.Object({
      kind: Type.String({ enum: ['image', 'video'], description: 'image or video' }),
      prompt: Type.String({ minLength: 1, description: 'Visual generation prompt' }),
      model: Type.Optional(Type.String({ description: 'provider/model; empty for auto' })),
      sourceImages: Type.Optional(
        Type.Array(Type.String({ minLength: 1 }), {
          maxItems: 8,
          description: 'Local image paths to edit',
        }),
      ),
      maskPath: Type.Optional(Type.String({ minLength: 1, description: 'Optional PNG mask path' })),
      outputName: Type.Optional(Type.String({ description: 'Output name without extension' })),
      aspectRatio: optionalStringEnum(['1:1', '16:9', '9:16', '4:3', '3:4']),
      size: optionalStringEnum([
        '1024x1024',
        '1536x1024',
        '1024x1536',
        '1280x720',
        '720x1280',
        '1792x1024',
        '1024x1792',
      ]),
      imageSize: optionalStringEnum(['1K', '2K', '4K']),
      resolution: optionalStringEnum(['720p', '1080p', '4k']),
      durationSeconds: Type.Optional(Type.Number({ enum: [4, 8, 12] })),
      quality: optionalStringEnum(['auto', 'low', 'medium', 'high', 'standard', 'hd']),
      outputFormat: optionalStringEnum(['png', 'jpeg', 'webp']),
    }),
    async execute(_toolCallId, params, signal, onUpdate) {
      if (!visualGenerationService) throw new Error('Visual generation service is not initialized.')
      const result = await visualGenerationService.generate(
        { ...params, cwd },
        {
          signal,
          onProgress: (message) => onUpdate?.({ content: [{ type: 'text', text: message }] }),
        },
      )
      try {
        await onGeneratedFile?.(result)
      } catch {
        // Asset indexing must not discard a successfully generated file.
      }
      return {
        content: [
          {
            type: 'text',
            text: `${result.operation === 'edit' ? 'Image edited' : result.kind === 'video' ? 'Video generated' : 'Image generated'}.\nFile: ${result.path}\nProvider: ${result.providerName}\nModel: ${result.modelName}`,
          },
        ],
        details: result,
      }
    },
  })
}
