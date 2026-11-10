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
  'batch',
  'finish',
]
// batch 里能用的动作：截图和结束不行（截图在最后自动有，结束要单独一步）。
const STEP_ACTIONS = ACTIONS.filter((value) => !['screenshot', 'batch', 'finish'].includes(value))
const MAX_STEPS = 10
// 一次调用里打字和等待的总量，和单个动作的上限一样：太长的一次调用会超出等待结果的时间。
const MAX_BATCH_TEXT = 5_000
const MAX_BATCH_WAIT_MS = 15_000
// 点击、拖动、滚动之后屏幕会变（可能弹出别的窗口、焦点换了地方），点没点对要看了才知道：
// 同一批里这些动作之后只能再等一等，不能接着打字、按键或再点，不然点歪时字会打进别的窗口。
const POINTING_ACTIONS = new Set(['click', 'double_click', 'right_click', 'drag', 'scroll'])

const coordinate = (description) => Type.Optional(Type.Number({ minimum: 0, description }))

function actionFields() {
  return {
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
  }
}

function checkBatch(steps) {
  if (!Array.isArray(steps) || steps.length === 0)
    throw new Error('batch needs steps: a list of the actions to run in order.')
  const text = steps.reduce((sum, step) => sum + String(step?.text || '').length, 0)
  if (text > MAX_BATCH_TEXT)
    throw new Error(
      `The steps type ${text} characters in total; keep one call under ${MAX_BATCH_TEXT}.`,
    )
  const wait = steps.reduce(
    (sum, step) => sum + (step?.action === 'wait' ? Number(step.waitMs) || 1_000 : 0),
    0,
  )
  const pointing = steps.findIndex((step) => POINTING_ACTIONS.has(step?.action))
  const blind =
    pointing < 0
      ? -1
      : steps.findIndex((step, index) => index > pointing && step?.action !== 'wait')
  if (blind >= 0) throw new Error(COMPUTER_USE.batchAfterPointing(blind + 1, steps[blind].action))
  if (wait > MAX_BATCH_WAIT_MS)
    throw new Error(
      `The steps wait ${wait} ms in total; keep one call under ${MAX_BATCH_WAIT_MS} ms.`,
    )
}

function resultText(params, result) {
  const { width, height } = result
  if (result.skipped === 'no_screenshot') return COMPUTER_USE.noScreenshot(width, height)
  if (result.failedStep) {
    const { index, action, error } = result.failedStep
    return COMPUTER_USE.stepFailed(index + 1, params.steps.length, action, error, width, height)
  }
  if (params.action === 'batch') return COMPUTER_USE.afterBatch(params.steps.length, width, height)
  return COMPUTER_USE.after(params.action, width, height)
}

export function createComputerUseTool({ browserSessionId, desktopComputerService }) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: manifest.description,
    promptSnippet: COMPUTER_USE.snippet,
    promptGuidelines: COMPUTER_USE.guidelines,
    parameters: Type.Object({
      action: Type.Union(ACTIONS.map((value) => Type.Literal(value))),
      ...actionFields(),
      steps: Type.Optional(
        Type.Array(
          Type.Object({
            action: Type.Union(STEP_ACTIONS.map((value) => Type.Literal(value))),
            ...actionFields(),
          }),
          { minItems: 1, maxItems: MAX_STEPS, description: COMPUTER_USE.params.steps },
        ),
      ),
    }),
    async execute(_toolCallId, params, signal) {
      if (!desktopComputerService) throw new Error('Computer use is not available here.')
      if (params.action === 'batch') checkBatch(params.steps)
      const result = await desktopComputerService.execute(browserSessionId, params, { signal })
      if (result.action === 'finish')
        return {
          content: [{ type: 'text', text: COMPUTER_USE.finished }],
          details: { action: 'finish' },
        }
      const details = { action: result.action, width: result.width, height: result.height }
      return {
        content: [
          { type: 'text', text: resultText(params, result) },
          { type: 'image', data: result.imageBase64, mimeType: 'image/jpeg' },
        ],
        details,
      }
    },
  })
}
