// 代理生成闭环（规格 5.2、计划 §5.4）：工作室“生成”、手机提示词底部表单与各宿主错误卡的 [让代理修复]
// 都走这一条路——先 POST …/components/:id/session 建 / 复用这个组件的绑定会话（cwd 是组件目录），
// 再经看板的 sendBoardTaskMessage 发消息：会话为空时以 '/skill:pisper-component ' 开头（后面是空格），
// 流由看板 store 持有，会话随即出现在侧栏“运行中”。代理写完文件后 component_changed 触发热重载。
// 同一个组件同一时刻只交一次（重复点击直接忽略）；最近一次交给代理的会话记在本页内存里，
// 提示词卡片据此显示进度与 [打开会话]（刷新即忘，绑定关系本身在 Runtime 的组件状态里）。
import { useCallback, useMemo, useSyncExternalStore } from 'react'
import { useI18n } from '@/app/use-i18n'
import { sendBoardTaskMessage } from '@/features/board/public-launch'
import {
  componentErrorCode,
  requestComponentAgentSession,
  type ComponentRow,
} from '@/features/custom-ui/custom-ui-api'
import { componentErrorText } from './error-text'
import { componentDisplayName } from './studio-model'

export const AGENT_PROMPT_MAX = 4000

export type ComponentAgentKind = 'generate' | 'fix'
export type ComponentAgentResult = { sessionId: string; queued: boolean }

type AgentState = { pending: ReadonlySet<string>; sessions: ReadonlyMap<string, string> }

let state: AgentState = { pending: new Set(), sessions: new Map() }
const listeners = new Set<() => void>()

function update(next: Partial<AgentState>) {
  state = { ...state, ...next }
  for (const listener of [...listeners]) listener()
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** 这个组件是否正在交给代理，以及本页里最近一次交给代理的会话 id。 */
export function useComponentAgentState(componentId: string) {
  const pending = useSyncExternalStore(subscribe, () => state.pending.has(componentId))
  const sessionId = useSyncExternalStore(subscribe, () => state.sessions.get(componentId) ?? '')
  return { pending, sessionId }
}

/** 交给代理：建 / 复用绑定会话并发消息。失败抛出原始错误（调用方换成文案）。 */
export async function askComponentAgent(
  componentId: string,
  prompt: string,
): Promise<ComponentAgentResult> {
  const session = await requestComponentAgentSession(componentId)
  return sendBoardTaskMessage({
    sessionId: session.sessionId,
    prompt: prompt.slice(0, AGENT_PROMPT_MAX),
    commandWhenEmpty: session.command,
    created: session.created ? session.session : null,
  })
}

export function useComponentAgent(notify: (message: string) => void) {
  const { t, language } = useI18n()
  return useMemo(() => {
    const send = async (
      row: ComponentRow,
      prompt: string,
      kind: ComponentAgentKind,
    ): Promise<ComponentAgentResult | null> => {
      const text = prompt.trim()
      if (!text || state.pending.has(row.id)) return null
      update({ pending: new Set(state.pending).add(row.id) })
      const name = componentDisplayName(row, language)
      try {
        const result = await askComponentAgent(row.id, text)
        update({ sessions: new Map(state.sessions).set(row.id, result.sessionId) })
        notify(
          result.queued
            ? t('custom-ui:agent.queued', { name })
            : kind === 'fix'
              ? t('custom-ui:agent.fixSent', { name })
              : t('custom-ui:agent.generateSent', { name }),
        )
        return result
      } catch (error) {
        const code = componentErrorCode(error)
        notify(
          code
            ? `${t('custom-ui:agent.failed')} ${componentErrorText(code, t)}`
            : t('custom-ui:agent.failed'),
        )
        return null
      } finally {
        const pending = new Set(state.pending)
        pending.delete(row.id)
        update({ pending })
      }
    }
    return {
      generate: (row: ComponentRow, prompt: string) => send(row, prompt, 'generate'),
      // 错误卡的 [让代理修复]：提示词固定为“修复以下错误：<摘要>”。
      fix: (row: ComponentRow, summary: string) =>
        send(row, t('custom-ui:agent.fixPrompt', { summary }), 'fix'),
    }
  }, [language, notify, t])
}

/** 宿主错误卡的 [让代理修复]：交给这个组件的绑定会话，提示由各宿主自己的 notify 显示。 */
export function useComponentFix(row: ComponentRow, notify: (message: string) => void) {
  const agent = useComponentAgent(notify)
  return useCallback((summary: string) => void agent.fix(row, summary), [agent, row])
}
