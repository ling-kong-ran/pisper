// Computer Use 的前端宿主：把 Runtime 的指令转给原生层，把截图交回去。逻辑都在原生层，这里只做转发，
// 不用定时器：开始后主窗口是最小化的，前端的定时器会被限速。
import type { DesktopComputerAction, DesktopComputerBridge } from '@/types/update'
import { connectDesktopHost, reportDesktopHostResult } from './desktop-host-connection'

const ENDPOINT = '/api/desktop-computer/host'
// 动作之后等界面反应过来再截图。
const SETTLE_MS = 450
// 批量动作里两步之间等一下（比如点开搜索框之后再打字），比截图前等得短。
const STEP_SETTLE_MS = 200
const MAX_WAIT_MS = 15_000
// 要指着屏幕上某处的动作：坐标来自最近一张截图，还没截过图就没有坐标可言。
const POINTING = new Set(['move', 'click', 'double_click', 'right_click', 'drag', 'scroll'])

type Labels = { title: string; hint: string; stop: string }

const asNumber = (value: unknown) => (typeof value === 'number' ? value : NaN)
const asString = (value: unknown) => (typeof value === 'string' ? value : '')

// 指令来自 Runtime，这里只认已知字段，坐标范围由原生层按最近一张截图检查。
export function computerActionFromCommand(
  command: Record<string, unknown>,
): DesktopComputerAction | null {
  const x = asNumber(command.x)
  const y = asNumber(command.y)
  switch (command.action) {
    case 'move':
      return { type: 'move', x, y }
    case 'click':
      return { type: 'click', x, y }
    case 'double_click':
      return { type: 'click', x, y, count: 2 }
    case 'right_click':
      return { type: 'click', x, y, button: 'right' }
    case 'drag':
      return { type: 'drag', x, y, toX: asNumber(command.toX), toY: asNumber(command.toY) }
    case 'scroll':
      return {
        type: 'scroll',
        x,
        y,
        direction: asString(command.direction) || 'down',
        amount: typeof command.amount === 'number' ? command.amount : undefined,
      }
    case 'type':
      return { type: 'type', text: asString(command.text) }
    case 'key':
      return { type: 'key', keys: asString(command.keys) }
    default:
      return null
  }
}

type ComputerDevice = Pick<DesktopComputerBridge, 'screenshot' | 'act'>

/**
 * 执行一条已开始操作的指令：截图、等待、单个动作，或一次按顺序执行的几步（batch，只在最后截图）。
 * 还没截过图就要点屏幕时什么都不做，先截一张交回。stopped 为真时（用户按了停止）不再往下做。
 */
export async function runComputerCommand(
  computer: ComputerDevice,
  command: Record<string, unknown>,
  screen: { seen: boolean },
  stopped: () => boolean,
) {
  const action = asString(command.action)
  const shot = async (settleMs: number) => {
    const image = await computer.screenshot(settleMs)
    screen.seen = true
    return image
  }
  // 等待也交给原生层计时（截一张不用的图）：主窗口最小化时前端的定时器会被限速。
  const pause = (waitMs: unknown) => {
    const value = asNumber(waitMs)
    return shot(Number.isFinite(value) ? Math.min(MAX_WAIT_MS, value) : 1_000)
  }
  if (action === 'screenshot') return shot(0)
  if (action === 'wait') return pause(command.waitMs)
  const batch = action === 'batch'
  const steps = (batch ? (Array.isArray(command.steps) ? command.steps : []) : [command]).map(
    (step) => (step && typeof step === 'object' ? (step as Record<string, unknown>) : {}),
  )
  if (!steps.length) throw new Error('batch needs at least one step.')
  // 没截过图就要点屏幕：什么都不做，先把屏幕交回去，模型下一轮就有坐标了。
  if (!screen.seen && steps.some((step) => POINTING.has(asString(step.action))))
    return { ...(await shot(0)), skipped: 'no_screenshot' }
  for (const [index, step] of steps.entries()) {
    if (stopped()) throw new Error('The user stopped computer use.')
    const stepAction = asString(step.action)
    try {
      if (batch && stepAction === 'wait') {
        await pause(step.waitMs)
        continue
      }
      const native = computerActionFromCommand(step)
      if (!native) throw new Error(`Unsupported computer action: ${stepAction}`)
      if (index > 0) await computer.screenshot(STEP_SETTLE_MS)
      await computer.act(native)
    } catch (error) {
      // 单个动作照旧报错；批量里某一步失败时停在这一步，把现在的屏幕和原因交回去，不用再截一轮。
      if (!batch || stopped()) throw error
      return {
        ...(await shot(SETTLE_MS)),
        failedStep: {
          index,
          action: stepAction,
          error: error instanceof Error ? error.message : String(error),
        },
      }
    }
  }
  return shot(SETTLE_MS)
}

function beginFailure(result: Awaited<ReturnType<DesktopComputerBridge['begin']>>) {
  if (result.status === 'permission_required') {
    const names = result.missingPermissions
      .map((name) => (name === 'screen' ? 'Screen Recording' : 'Accessibility'))
      .join(' and ')
    return `macOS needs the ${names} permission for Pisper. The system prompt has been opened. Ask the user to allow it in System Settings > Privacy & Security, restart Pisper if macOS asks, then try again.`
  }
  return result.reason || 'Computer use is not available on this system.'
}

export function startAgentComputerHost(labels: () => Labels) {
  const computer = window.pisperDesktop?.computer
  if (!computer) return () => {}
  const controller = new AbortController()
  let activeSession = ''
  // 这次开始操作以来截过图没有（原生层的坐标换算也是从第一张截图才有的）。
  const screen = { seen: false }
  // 用户按停的会话：这一轮结束（release）前，还在路上的指令不能把遮罩又开起来。
  const stoppedSessions = new Set<string>()
  let queue: Promise<unknown> = Promise.resolve()

  const end = () => {
    activeSession = ''
    return computer.end().catch(() => {})
  }

  const run = async (command: Record<string, unknown>) => {
    const sessionId = asString(command.sessionId)
    const action = asString(command.action)
    if (action === 'finish') {
      if (activeSession === sessionId) await end()
      return {}
    }
    if (stoppedSessions.has(sessionId)) throw new Error('The user stopped computer use.')
    if (activeSession !== sessionId) {
      const result = await computer.begin(sessionId, labels(), () => {
        // 用户按了 Esc 或点了停止：原生层已经收起遮罩，这里停掉这一轮。
        activeSession = ''
        stoppedSessions.add(sessionId)
        void fetch(`/api/sessions/${encodeURIComponent(sessionId)}/abort`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: '{}',
        }).catch(() => {})
      })
      if (result.status !== 'started') throw new Error(beginFailure(result))
      activeSession = sessionId
      screen.seen = false
    }
    return runComputerCommand(
      computer,
      command,
      screen,
      () => stoppedSessions.has(sessionId) || activeSession !== sessionId,
    )
  }

  const enqueue = (command: Record<string, unknown>) => {
    const requestId = asString(command.requestId)
    queue = queue.then(async () => {
      try {
        const result = await run(command)
        await reportDesktopHostResult(ENDPOINT, requestId, { ok: true, result })
      } catch (error) {
        await reportDesktopHostResult(ENDPOINT, requestId, {
          ok: false,
          error: error instanceof Error ? error.message : String(error),
        })
      }
    })
  }

  // 远程工作区窗口没有这组命令的权限：连上来只会让 Agent 以为能用。
  void computer.end().then(
    () =>
      connectDesktopHost(
        ENDPOINT,
        controller.signal,
        (event, data) => {
          const record = (data && typeof data === 'object' ? data : {}) as Record<string, unknown>
          if (event === 'command' && asString(record.requestId)) enqueue(record)
          else if (event === 'release' || event === 'cancel') {
            const sessionId = asString(record.sessionId)
            if (event === 'release') stoppedSessions.delete(sessionId)
            if (sessionId === activeSession) void end()
          }
        },
        // Runtime 断了就没人会让它结束了，不能把遮罩留在屏幕上。
        () => {
          if (activeSession) void end()
        },
      ),
    () => {},
  )

  return () => {
    controller.abort()
    if (activeSession) void end()
  }
}
