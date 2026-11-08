// Computer Use 工具：看屏幕、动鼠标、敲键盘。桌面端由原生层执行，模型用的坐标就是截图上的像素。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { COMPUTER_USE } from '../../prompts/tools/computer-use.mjs'

export const manifest = {
  id: 'computer_use',
  name: 'Computer Use',
  category: 'system',
  risk: 'high',
  description: COMPUTER_USE.description,
  scope: 'The whole screen of the computer running the Pisper desktop app',
  capability: 'Take screenshots, move, click, drag, scroll, type, and press keys',
  source: 'app',
}

const ACTIONS = [
  'screenshot',
  'move',
  'click',
  'double_click',
  'right_click',
  'drag',
  'scroll',
  'type',
  'key',
  'wait',
  'finish',
]

const coordinate = (description) => Type.Optional(Type.Number({ minimum: 0, description }))

export function createComputerUseTool({ browserSessionId, desktopComputerService }) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: manifest.description,
    promptSnippet: COMPUTER_USE.snippet,
    promptGuidelines: COMPUTER_USE.guidelines,
    parameters: Type.Object({
      action: Type.Union(ACTIONS.map((value) => Type.Literal(value))),
      x: coordinate(COMPUTER_USE.params.x),
      y: coordinate(COMPUTER_USE.params.y),
      toX: coordinate(COMPUTER_USE.params.toX),
      toY: coordinate(COMPUTER_USE.params.toY),
      direction: Type.Optional(
        Type.Union(['up', 'down', 'left', 'right'].map((value) => Type.Literal(value))),
      ),
      amount: Type.Optional(
        Type.Integer({ minimum: 1, maximum: 50, description: COMPUTER_USE.params.amount }),
      ),
      text: Type.Optional(Type.String({ maxLength: 5_000, description: COMPUTER_USE.params.text })),
      keys: Type.Optional(Type.String({ maxLength: 100, description: COMPUTER_USE.params.keys })),
      waitMs: Type.Optional(
        Type.Integer({ minimum: 0, maximum: 15_000, description: COMPUTER_USE.params.waitMs }),
      ),
    }),
    async execute(_toolCallId, params, signal) {
      if (!desktopComputerService) throw new Error('Computer use is not available here.')
      const result = await desktopComputerService.execute(browserSessionId, params, { signal })
      if (result.action === 'finish')
        return {
          content: [{ type: 'text', text: COMPUTER_USE.finished }],
          details: { action: 'finish' },
        }
      const details = { action: result.action, width: result.width, height: result.height }
      return {
        content: [
          {
            type: 'text',
            text: `Screenshot after ${result.action}: ${result.width}x${result.height}.`,
          },
          { type: 'image', data: result.imageBase64, mimeType: 'image/jpeg' },
        ],
        details,
      }
    },
  })
}
