// 会话页（桌面布局）的外壳契约：路由传入的页面形态，以及页头与检查器之间的开关通道。
// 只放类型与上下文，不导入任何组件，页头、工作区与检查器都可以依赖它而不形成环。
import { createContext, type ReactNode } from 'react'
import type { SessionInspectorTab } from './session-inspector-store'

// 会话页由应用壳注入：有值时单会话视图换成“页头 + 对话 + 检查器”的会话页布局，
// 没有值（旧入口）时保持原来的单会话视图。
export type SessionPageChrome = {
  // ≤1279 宽或 ≤760 高：检查器默认关闭，改为右侧滑出的 420px 覆盖层。
  compact: boolean
  // 手机 App 平板：始终用覆盖层（规格 7.6）。
  overlayOnly: boolean
  // 手机外壳（phoneShell）的会话页（规格 v2 第 7.2 节）：顶栏 56 + 对话，检查器改为底部表单。
  // 第 4 期的代理条退役（并行的代理在抽屉里）；没有改动时顶栏右侧的 ✎ 交给应用壳打开派发表单。
  phone?: SessionPhoneChrome
  // 检查器“+”里去组件页设置“会话面板”放置。
  onOpenComponents?: () => void
  // 组件标签（“会话面板”放置，规格 4.4）：由应用壳按放置实例构造；聊天领域只认这份类型契约，不依赖组件系统。
  // 没有值（旧 Runtime、没有组件能力）时检查器只显示固定标签，“+”里是去组件页的提示。
  componentTabs?: SessionComponentTabs
  // 归档后离开会话页，回到主页（会话做主页：上次会话已不可用，显示新任务草稿）。
  onArchived?: (sessionId: string) => void
  // “找不到这个会话”的出口：回到主页（上次停留的会话或新任务草稿）；旧入口没有时回看板。
  onHome?: () => void
}

export type SessionComponentTab = { id: string; label: string }

export type SessionComponentTabs = {
  // 当前会话（按项目目录）可见的组件标签，id 是放置实例 id。
  tabs: (cwd: string) => SessionComponentTab[]
  // 标签内容：铺满检查器的标签内容区。
  render: (id: string, context: { sessionId: string; cwd: string }) => ReactNode
  // “+”弹层里的组件选择器；放好后 onAdded 切到新标签并关掉弹层。
  addMenu?: (context: { cwd: string; onAdded: (id: string) => void }) => ReactNode
}

export type SessionPhoneChrome = {
  onNewTask: () => void
}

export const SessionPageChromeContext = createContext<SessionPageChrome | null>(null)

export type SessionInspectorControl = {
  // 检查器当前是否可见（并排展开或覆盖层打开）。
  open: boolean
  // sheet：手机外壳的底部表单。
  mode: 'aside' | 'overlay' | 'sheet'
  panelId: string
  toggle: () => void
  show: (tab: SessionInspectorTab) => void
}

export const SessionInspectorContext = createContext<SessionInspectorControl | null>(null)

// 会话页转录的代理信息行（规格 4.3：Logo、模型、第 N 轮、审批模式）：模型与审批模式只代表会话的当前设置，
// 所以只挂在最新一条回复上；历史回复没有逐条记录，不拿当前值冒充。没有值（手机外壳、旧入口）时保持原来的抬头。
export type TranscriptMeta = {
  model: string
}

export const TranscriptMetaContext = createContext<TranscriptMeta | null>(null)
