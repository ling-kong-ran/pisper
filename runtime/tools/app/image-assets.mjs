import { Type } from 'typebox'
import { Compile } from 'typebox/compile'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { normalizeImageFrameEdits } from '../../../shared/image-frame-edits.mjs'
import { parseWorkflowMedia } from '../../../shared/workflow-inputs.mjs'
import {
  normalizeImageSettings,
  parseImageOutput,
  imageOperationError,
} from '../../../shared/image-operations.mjs'

export const manifest = {
  id: 'image_assets',
  name: 'Image Assets',
  category: 'visual',
  risk: 'high',
  description:
    'Generate action frames, remove backgrounds locally, split, manually edit, and export image assets.',
  scope: 'Imported workspace images and managed image assets',
  capability: 'Use the same image-processing engine as workflows and the game assets workbench',
  source: 'app',
}

const number = (min, max) => Type.Number({ minimum: min, maximum: max })
const optionalNumber = (min, max) => Type.Optional(number(min, max))
const mediaSchema = Type.Object(
  {
    id: Type.String({ pattern: '^[a-zA-Z0-9][a-zA-Z0-9_-]{0,79}$' }),
    name: Type.String({ minLength: 1, maxLength: 160 }),
    mimeType: Type.String({ enum: ['image/png', 'image/jpeg', 'image/webp'] }),
    size: Type.Integer({ minimum: 1, maximum: 8 * 1024 * 1024 }),
  },
  { additionalProperties: false },
)
const transformProperties = {
  x: optionalNumber(-4096, 4096),
  y: optionalNumber(-4096, 4096),
  rotation: optionalNumber(-360, 360),
  scale: optionalNumber(0.05, 8),
  opacity: optionalNumber(0, 1),
  durationMs: Type.Optional(Type.Integer({ minimum: 16, maximum: 10000 })),
}
const frameSchema = Type.Object(
  {
    media: mediaSchema,
    width: Type.Integer({ minimum: 1, maximum: 4096 }),
    height: Type.Integer({ minimum: 1, maximum: 4096 }),
    durationMs: Type.Integer({ minimum: 16, maximum: 10000 }),
    action: Type.String({ maxLength: 160 }),
    direction: Type.String({ maxLength: 16 }),
    columns: Type.Integer({ minimum: 1, maximum: 16 }),
    rows: Type.Integer({ minimum: 1, maximum: 16 }),
    frameCount: Type.Integer({ minimum: 1, maximum: 256 }),
  },
  { additionalProperties: false },
)
const parameters = Type.Object(
  {
    operation: Type.String({
      enum: [
        'input',
        'background',
        'inpaint',
        'generate',
        'frames',
        'transform',
        'preview',
        'export',
        'edit',
      ],
    }),
    sourceImage: Type.Optional(
      Type.String({
        minLength: 1,
        maxLength: 4096,
        description:
          'Image path inside the current workspace. Imported into managed media before processing; no URLs.',
      }),
    ),
    source: Type.Optional(mediaSchema),
    images: Type.Optional(
      Type.Array(frameSchema, {
        minItems: 1,
        maxItems: 512,
        description: 'Frames returned by a previous image_assets call.',
      }),
    ),
    prompt: Type.Optional(Type.String({ maxLength: 16000 })),
    model: Type.Optional(
      Type.String({
        maxLength: 240,
        description:
          'provider/model; omit for the configured image model. Model IDs may include additional slashes.',
      }),
    ),
    settings: Type.Optional(
      Type.Object(
        {
          method: Type.Optional(
            Type.String({
              enum: ['color', 'model'],
              description:
                'Background removal: color key or downloaded local segmentation model. Neither uses a remote image model.',
            }),
          ),
          colors: Type.Optional(
            Type.Array(Type.String({ pattern: '^#[0-9a-fA-F]{6}$' }), { maxItems: 8 }),
          ),
          tolerance: optionalNumber(0, 255),
          softness: optionalNumber(0, 64),
          edgeConnected: Type.Optional(Type.Boolean()),
          region: Type.Optional(
            Type.Object(
              { x: number(0, 99), y: number(0, 99), width: number(1, 100), height: number(1, 100) },
              { additionalProperties: false },
            ),
          ),
          action: Type.Optional(Type.String({ maxLength: 160 })),
          frameCount: Type.Optional(Type.Integer({ minimum: 1, maximum: 16 })),
          directions: Type.Optional(
            Type.Array(Type.String({ enum: ['S', 'SW', 'W', 'NW', 'N', 'NE', 'E', 'SE'] }), {
              minItems: 1,
              maxItems: 8,
              uniqueItems: true,
            }),
          ),
          columns: Type.Optional(Type.Integer({ minimum: 1, maximum: 16 })),
          rows: Type.Optional(Type.Integer({ minimum: 1, maximum: 16 })),
          durationMs: transformProperties.durationMs,
          trim: Type.Optional(Type.Boolean()),
          align: Type.Optional(Type.String({ enum: ['center', 'bottom-center', 'none'] })),
          padding: Type.Optional(Type.Integer({ minimum: 0, maximum: 64 })),
          maxFrameSize: Type.Optional(Type.Integer({ minimum: 16, maximum: 1024 })),
          frameOrder: Type.Optional(
            Type.Array(Type.Integer({ minimum: 0, maximum: 511 }), {
              maxItems: 512,
              uniqueItems: true,
            }),
          ),
          transforms: Type.Optional(
            Type.Array(
              Type.Object(
                {
                  index: Type.Integer({ minimum: 0, maximum: 511 }),
                  ...transformProperties,
                  enabled: Type.Optional(Type.Boolean()),
                },
                { additionalProperties: false },
              ),
              { maxItems: 512 },
            ),
          ),
          filename: Type.Optional(Type.String({ maxLength: 100 })),
        },
        { additionalProperties: false },
      ),
    ),
    edits: Type.Optional(
      Type.Object(
        {
          frames: Type.Array(
            Type.Object(
              {
                sourceIndex: Type.Integer({ minimum: 0, maximum: 511 }),
                ...transformProperties,
                eraseStrokes: Type.Optional(
                  Type.Array(
                    Type.Object(
                      {
                        points: Type.Array(
                          Type.Object(
                            { x: number(0, 1), y: number(0, 1) },
                            { additionalProperties: false },
                          ),
                          { minItems: 1, maxItems: 512 },
                        ),
                        radius: number(0.001, 0.25),
                        restore: Type.Optional(Type.Boolean()),
                      },
                      { additionalProperties: false },
                    ),
                    { maxItems: 64 },
                  ),
                ),
              },
              { additionalProperties: false },
            ),
            { minItems: 1, maxItems: 512 },
          ),
        },
        { additionalProperties: false },
      ),
    ),
  },
  { additionalProperties: false },
)
const validator = Compile(parameters)

/** @param {string|undefined} value */
function selectedModel(value) {
  const normalized = value?.trim()
  if (!normalized) return undefined
  const separator = normalized.indexOf('/')
  if (separator < 1 || separator === normalized.length - 1 || /[\r\n]/.test(normalized))
    throw imageOperationError('workflow_image_invalid')
  return { provider: normalized.slice(0, separator), model: normalized.slice(separator + 1) }
}

/** 工具只接受受控资源引用；文件权限与 Agent 开关由注入的窄接口检查。 */
export function createImageAssetsTool({ cwd, imageAssets, onGeneratedFile }) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: manifest.description,
    promptSnippet: 'Process game image assets and animation frames',
    promptGuidelines: [
      'Import a workspace image with sourceImage or reuse managed media / frames from a prior result. Never fabricate media IDs.',
      'Only generate calls a configured image model. Background, inpaint, frames, transform, edit and export are local operations; downloaded engines may be required.',
      'Ask the user to inspect animation quality. Use edit to reorder, duplicate, remove, transform or erase individual frames without regenerating them.',
      'Export writes a PNG atlas and frame-metadata JSON under workspace/generated/image-assets. Use the paths returned in files; other operations return managed media references.',
      'Do not enable this tool yourself. It is available to agents only when the user enables Image Assets in Plugins; workbench and workflow usage is independent.',
    ],
    parameters,
    async execute(_toolCallId, params, signal) {
      if (!imageAssets) throw imageOperationError('image_tools_unavailable')
      if (
        !validator.Check(params) ||
        (params.sourceImage && params.source) ||
        (params.edits && params.operation !== 'edit')
      )
        throw imageOperationError('workflow_image_invalid')
      signal?.throwIfAborted()
      // 在导入文件产生持久化副作用之前完成所有纯输入校验。
      const model = selectedModel(params.model)
      const settings = normalizeImageSettings(params.settings)
      const edits = params.operation === 'edit' ? normalizeImageFrameEdits(params.edits) : undefined
      const images = params.images
        ? parseImageOutput({
            type: 'workflow-images',
            version: 1,
            frames: params.images,
          }).frames
        : undefined
      const source = params.sourceImage
        ? parseWorkflowMedia(await imageAssets.importImage(cwd, params.sourceImage, { signal }))
        : params.source
          ? parseWorkflowMedia(params.source)
          : undefined
      let result = await imageAssets.execute(
        {
          operation: params.operation,
          settings,
          source,
          images,
          prompt: params.prompt,
          model,
          edits,
        },
        { signal },
      )
      if (params.operation === 'export') {
        const exported = await imageAssets.exportImages(cwd, result.output, { signal })
        result = { ...result, files: exported.files }
        for (const file of exported.files) {
          try {
            await onGeneratedFile?.(file)
          } catch {
            // 资源索引失败不能丢失已经写入工作区的素材路径。
          }
        }
      }
      return {
        content: [{ type: 'text', text: JSON.stringify(result) }],
        details: result,
      }
    },
  })
}
