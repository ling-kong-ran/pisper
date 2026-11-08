// 视觉生成工具：图像/视频生成与编辑，产物写入工作区 generated/visuals。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { GENERATE_VISUAL } from '../../prompts/tools/visual-generate.mjs'

export const manifest = {
  id: 'generate_visual',
  name: 'Visual Generate',
  category: 'visual',
  risk: 'high',
  description: GENERATE_VISUAL.description,
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
    promptSnippet: GENERATE_VISUAL.snippet,
    promptGuidelines: GENERATE_VISUAL.guidelines,
    parameters: Type.Object({
      kind: Type.String({ enum: ['image', 'video'] }),
      prompt: Type.String({ minLength: 1, description: GENERATE_VISUAL.params.prompt }),
      model: Type.Optional(Type.String({ description: GENERATE_VISUAL.params.model })),
      sourceImages: Type.Optional(
        Type.Array(Type.String({ minLength: 1 }), {
          maxItems: 8,
          description: GENERATE_VISUAL.params.sourceImages,
        }),
      ),
      maskPath: Type.Optional(
        Type.String({ minLength: 1, description: GENERATE_VISUAL.params.maskPath }),
      ),
      outputName: Type.Optional(Type.String({ description: GENERATE_VISUAL.params.outputName })),
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
