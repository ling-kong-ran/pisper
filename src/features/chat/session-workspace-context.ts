// 会话页（桌面布局）的外壳契约：路由传入的页面形态，以及页头与检查器之间的开关通道。
// 只放类型与上下文，不导入任何组件，页头、工作区与检查器都可以依赖它而不形成环。
import { createContext } from 'react'
import type { SessionInspectorTab } from './session-inspector-store'

// 会话页由应用壳注入：有值时单会话视图换成“页头 44 + 对话 + 检查器”的会话页布局，
// 没有值（手机外壳、旧入口）时保持原来的单会话视图。
export type SessionPageChrome = {
  // ≤1279 宽或 ≤760 高：检查器默认关闭，改为右侧滑出的 420px 覆盖层。
  compact: boolean
  // 手机 App 平板：始终用覆盖层（规格 7.6）。
  overlayOnly: boolean
  // 检查器“+”里去组件页设置“会话面板”放置。
  onOpenComponents?: () => void
  // 归档后离开会话页，回到看板。
  onArchived?: (sessionId: string) => void
}

export const SessionPageChromeContext = createContext<SessionPageChrome | null>(null)

export type SessionInspectorControl = {
  // 检查器当前是否可见（并排展开或覆盖层打开）。
  open: boolean
  mode: 'aside' | 'overlay'
  panelId: string
  toggle: () => void
  show: (tab: SessionInspectorTab) => void
}

export const SessionInspectorContext = createContext<SessionInspectorControl | null>(null)

// 会话页转录的代理信息行（规格 4.3：Logo、模型、第 N 轮、审批模式）：模型与审批模式只代表会话的当前设置，
// 所以只挂在最新一条回复上；历史回复没有逐条记录，不拿当前值冒充。没有值（手机外壳、旧入口）时保持原来的抬头。
export type TranscriptMeta = {
  model: string
  approvalMode: string
}

export const TranscriptMetaContext = createContext<TranscriptMeta | null>(null)
