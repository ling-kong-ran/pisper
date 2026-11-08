// 浏览器自动化工具：导航/检查/交互/截图。桌面端操作用户看得见的右侧浏览器，其他环境是隔离的无头浏览器。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { BROWSER_AUTOMATION } from '../../prompts/tools/browser-automation.mjs'

export const manifest = {
  id: 'browser_automation',
  name: 'Browser Control',
  category: 'browser',
  risk: 'medium',
  description: BROWSER_AUTOMATION.description,
  scope: 'Browser for the current primary Agent chat; HTTP/HTTPS only',
  capability:
    'Navigate, inspect, click, type, wait, capture screenshots, and close the browser without arbitrary JavaScript',
  source: 'app',
}

const actionSchema = Type.Union(
  ['open', 'inspect', 'click', 'type', 'wait', 'screenshot', 'close'].map((value) =>
    Type.Literal(value),
  ),
)

function formatResult(result) {
  const compact = { ...result }
  if (compact.text?.length > 20_000) compact.text = compact.text.slice(0, 20_000)
  return JSON.stringify(compact, null, 2)
}

export function createBrowserAutomationTool({
  cwd,
  browserSessionId,
  browserAutomationService,
  onGeneratedFile,
}) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: manifest.description,
    promptSnippet: BROWSER_AUTOMATION.snippet,
    promptGuidelines: BROWSER_AUTOMATION.guidelines,
    parameters: Type.Object({
      action: actionSchema,
      url: Type.Optional(
        Type.String({ maxLength: 2_000, description: BROWSER_AUTOMATION.params.url }),
      ),
      selector: Type.Optional(
        Type.String({ maxLength: 500, description: BROWSER_AUTOMATION.params.selector }),
      ),
      text: Type.Optional(
        Type.String({ maxLength: 5_000, description: BROWSER_AUTOMATION.params.text }),
      ),
      submit: Type.Optional(Type.Boolean({ description: BROWSER_AUTOMATION.params.submit })),
      waitMs: Type.Optional(Type.Integer({ minimum: 0, maximum: 15_000 })),
      outputName: Type.Optional(
        Type.String({ maxLength: 120, description: BROWSER_AUTOMATION.params.outputName }),
      ),
      fullPage: Type.Optional(Type.Boolean({ description: BROWSER_AUTOMATION.params.fullPage })),
      width: Type.Optional(Type.Integer({ minimum: 640, maximum: 2560 })),
      height: Type.Optional(Type.Integer({ minimum: 480, maximum: 1600 })),
    }),
    async execute(_toolCallId, params, signal, onUpdate) {
      if (!browserAutomationService)
        throw new Error('Browser automation service is not initialized.')
      const result = await browserAutomationService.execute(browserSessionId, params, {
        cwd,
        signal,
        onProgress: (message) => onUpdate?.({ content: [{ type: 'text', text: message }] }),
      })
      if (result?.path) {
        try {
          await onGeneratedFile?.(result)
        } catch {}
      }
      return { content: [{ type: 'text', text: formatResult(result) }], details: result }
    },
  })
}
