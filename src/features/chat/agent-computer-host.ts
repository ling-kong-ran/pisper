// Computer Use 的前端宿主：把 Runtime 的指令转给原生层，把截图交回去。逻辑都在原生层，这里只做转发，
// 不用定时器：开始后主窗口是最小化的，前端的定时器会被限速。
import type { DesktopComputerAction, DesktopComputerBridge } from '@/types/update'
import { connectDesktopHost, reportDesktopHostResult } from './desktop-host-connection'

const ENDPOINT = '/api/desktop-computer/host'
// 动作之后等界面反应过来再截图。
const SETTLE_MS = 450
const MAX_WAIT_MS = 15_000

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
    }
    if (action === 'screenshot') return computer.screenshot(0)
    if (action === 'wait') {
      const waitMs = asNumber(command.waitMs)
      return computer.screenshot(Number.isFinite(waitMs) ? Math.min(MAX_WAIT_MS, waitMs) : 1_000)
    }
    const native = computerActionFromCommand(command)
    if (!native) throw new Error(`Unsupported computer action: ${action}`)
    await computer.act(native)
    return computer.screenshot(SETTLE_MS)
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
