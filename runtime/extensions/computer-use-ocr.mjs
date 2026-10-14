import { register } from 'tsx/esm/api'
import { Type } from 'typebox'
import { createTesseractOcrService } from '../services/tesseract-ocr-service.mjs'

// 官方包只提供 TypeScript Extension；运行时注册 tsx 使其 bridge 保持与 Pi Extension 相同的执行实现。
register()
const { executeObserve } = await import('@injaneity/pi-computer-use/src/bridge.ts')
const ocrService = createTesseractOcrService()

const language = Type.Optional(
  Type.Union([Type.Literal('eng'), Type.Literal('chi_sim'), Type.Literal('chi_sim+eng')]),
)

function imageContent(result) {
  return result?.content?.find((item) => item?.type === 'image' && item.data)?.data
}

export default function computerUseOcrExtension(pi) {
  pi.registerTool({
    name: 'ocr_ui',
    label: 'OCR UI',
    description:
      'Extract English, Simplified Chinese, or mixed Chinese-English text from the current UI screenshot.',
    promptSnippet:
      'Use after observe_ui when accessibility text is incomplete and visible text must be read.',
    parameters: Type.Object({
      root: Type.Optional(Type.String({ description: 'Exact @r ref issued by find_roots' })),
      language,
    }),
    async execute(toolCallId, params, signal, onUpdate, ctx) {
      const observed = await executeObserve(
        toolCallId,
        { root: params.root, mode: 'visual' },
        signal,
        onUpdate,
        ctx,
      )
      const data = imageContent(observed)
      if (!data) throw new Error('当前 UI 观察没有返回截图，无法执行 OCR。')
      const result = await ocrService.recognize(Buffer.from(data, 'base64'), {
        language: params.language || 'chi_sim+eng',
        signal,
      })
      return {
        content: [
          {
            type: 'text',
            text: `OCR (${result.language}, confidence ${result.confidence ?? 'unknown'}):\n${result.text || '(no text detected)'}`,
          },
        ],
        details: { ...observed.details, tool: 'ocr_ui', ocr: result },
      }
    },
  })
}
