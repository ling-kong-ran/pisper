// 使用真实聊天组件，所有消息、延迟请求与文件都由隔离夹具生成。
import { createRoot } from 'react-dom/client'
import { useState } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { FocusSession, type FocusSessionProps } from '@/features/chat/FocusSession'
import {
  reconcileOlderMessageWindow,
  restoreLatestMessageWindow,
} from '@/features/chat/use-live-session-sync'
import { SessionPageChromeContext } from '@/features/chat/session-workspace-context'
import { ensureBoardMessages } from '@/app/i18n'
import { DEFAULT_SESSION_STATE } from '@/lib/session-state'
import type { ChatMessage, EntityRecord } from '@/types/chat'
import '@/index.css'

// 一律按桌面会话页（/board/:id）的方式渲染——提供会话页外壳上下文（真实会话页头、会话页输入框），
// 并先注册会话页文案（与会话路由加载时一致）。A6 起非路由的旧单会话视图（TaskWorkspaceHeader 顶栏）已退役，
// ?layout=page 参数保留只为兼容旧链接，不再区分两种外观。
const search = new URLSearchParams(location.search)
// ?approvals=1：两条待批（一条命令守卫拦截的 critical、一条普通），核对内联批准卡与按住允许。
const withApprovals = search.get('approvals') === '1'
// ?questions=1：两组提问（先是两题含多选的一组，回答后是一题单选），核对问题卡的点选、提交与跳过。
const initialQuestions: EntityRecord[] =
  search.get('questions') === '1'
    ? [
        {
          id: 'q_fixture-multi-0001',
          createdAt: '2026-10-31T08:00:00.000Z',
          questions: [
            {
              header: '存储',
              question: '会话存储用哪种？',
              options: [{ label: 'SQLite', description: '单机' }, { label: 'Postgres' }],
              multiSelect: false,
            },
            {
              header: '测试',
              question: '要补哪些测试？',
              options: [{ label: '单测' }, { label: '冒烟' }, { label: '契约' }],
              multiSelect: true,
            },
          ],
        },
        {
          id: 'q_fixture-single-001',
          createdAt: '2026-10-31T08:01:00.000Z',
          questions: [
            {
              header: '分支',
              question: '提交到哪个分支？',
              options: [{ label: 'main' }, { label: 'release' }],
              multiSelect: false,
            },
          ],
        },
      ]
    : []
const initialApprovals: EntityRecord[] = withApprovals
  ? [
      {
        id: 'approval-critical',
        toolName: 'bash',
        args: { command: 'git push --force-with-lease origin fix/ci' },
        reason: '命令守卫拦截：改写远端分支。需要确认。',
      },
      { id: 'approval-standard', toolName: 'edit', args: { path: 'src/auth/login.ts' } },
    ]
  : []
// 运行中的工具组：一条已完成的读取、一条正在跑的测试（输出含通过与失败行）。
const runningFeed: EntityRecord[] = [
  { type: 'tool', id: 't1', name: 'read', args: { path: 'src/auth/login.ts' }, status: 'done' },
  {
    type: 'tool',
    id: 't2',
    name: 'bash',
    args: { command: 'npm test -- auth' },
    status: 'running',
    startedAt: new Date().toISOString(),
    output: '✓ login › accepts valid token\n✗ login › rejects expired token',
  },
]
// 运行结束后同一组操作都已完成：工具组应自动折叠成一行。
const finishedFeed: EntityRecord[] = runningFeed.map((item) => ({ ...item, status: 'done' }))
await ensureBoardMessages()
const pageChrome = { compact: false, overlayOnly: false }

const noop = () => {}
const markdown = `正文段落用于核对字距、换行与行高。Content baseline and wrapping.

第二段包含 **粗体**、中文与 English，及 \`inline_code\`。

# Heading 1
## Heading 2
### Heading 3
#### Heading 4
##### Heading 5
###### Heading 6

- 第一项
- 第二项

  列表中多段正文。

| Item | Value |
| --- | --- |
| table | 768px |

\`\`\`js
const value = 'fixed specimen'
\`\`\``
const specimens: ChatMessage[] = [
  { id: 'user', role: 'user', text: '用户第一段。\n\n用户第二段。' },
  // thinkingMs：A9 的思考用时，信息行应写“思考 6.2s”。
  { id: 'agent', role: 'agent', text: markdown, thinkingMs: 6200 },
]
const rows = (start: number, count: number): ChatMessage[] =>
  Array.from({ length: count }, (_, i) => ({
    id: `m${start + i}`,
    role: i % 2 ? 'agent' : 'user',
    text: `Message ${start + i}\n\n` + '固定阅读锚点。Stable reading anchor. '.repeat(3),
  }))
const evidence = {
  sends: 0,
  queues: 0,
  aborts: 0,
  attachments: 0,
  behaviors: [] as (string | null)[],
  withdrawn: [] as string[],
  approvals: [] as [string, boolean][],
  answers: [] as [string, unknown][],
}
const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })

export function Fixture() {
  const [sessionId, setSessionId] = useState('fixture-a')
  const [streaming, setStreaming] = useState(false)
  const [queued, setQueued] = useState<EntityRecord[]>([])
  const [ran, setRan] = useState(false)
  const [approvals, setApprovals] = useState<EntityRecord[]>(initialApprovals)
  const [questions, setQuestions] = useState<EntityRecord[]>(initialQuestions)
  const [history, setHistory] = useState({ ...DEFAULT_SESSION_STATE, messages: specimens })
  const loadOlder = () => {
    const start = Math.max(
      0,
      (history.historyWindow?.messageStart ?? history.messageStart ?? 0) - 50,
    )
    setHistory((current) => ({
      ...reconcileOlderMessageWindow(current, {
        messages: rows(start, 50),
        pageInfo: { start, hasMore: start > 0, nextCursor: start > 0 ? String(start) : null },
      }),
    }))
    return Promise.resolve(true)
  }
  const props: FocusSessionProps = {
    session: { id: sessionId, name: 'Replica fixture' },
    messages: history.historyWindow?.messages || history.messages,
    messageStart: history.historyWindow?.messageStart ?? history.messageStart,
    hasOlder: history.historyWindow?.hasOlder ?? history.hasOlder,
    hasNewer: Boolean(history.historyWindow),
    onLoadOlder: loadOlder,
    onLoadLatest: () => {
      setHistory(restoreLatestMessageWindow)
      return Promise.resolve()
    },
    model: 'fixture/model',
    availableModels: [{ key: 'fixture/model', label: 'Fixture Model', providerName: 'Fixture' }],
    executionMode: 'default',
    // 结束后实时活动只剩 tools（与会话快照一致），工具组仍挂在最新回复上。
    tools: !streaming && ran ? finishedFeed : [],
    approvals,
    activityFeed: streaming ? runningFeed : [],
    queuedInputs: queued,
    runStartedAt: streaming ? new Date(Date.now() - 192_000).toISOString() : null,
    streaming,
    onModelChange: noop,
    onThinkingLevelChange: noop,
    onExecutionModeChange: () => true,
    onRunModeChange: () => true,
    questions,
    onAnswerQuestion: (questionId, body) => {
      evidence.answers.push([questionId, body])
      setQuestions((current) => current.filter((item) => item.id !== questionId))
      return Promise.resolve()
    },
    onApproval: (id, approved) => {
      evidence.approvals.push([id, approved])
      setApprovals((current) => current.filter((item) => item.id !== id))
    },
    onWorkspace: noop,
    onRename: noop,
    onBranchFromHere: noop,
    onCreateChildSession: noop,
    onRetryLastTurn: noop,
    onSend: (_text, attachments, _goal, _team, _budget, _invocation, onAccepted) => {
      evidence.sends += 1
      evidence.attachments = attachments.length
      return new Promise<boolean>((resolve) =>
        Object.assign(window, {
          settleSend: (accepted: boolean) => {
            if (accepted) {
              onAccepted?.()
              setStreaming(true)
              setRan(true)
            }
            resolve(accepted)
          },
        }),
      )
    },
    onQueue: (text, _attachments, behavior) => {
      evidence.queues += 1
      evidence.behaviors.push(behavior ?? null)
      setQueued((current) => [...current, { id: `q${evidence.queues}`, text, behavior }])
      return Promise.resolve(true)
    },
    onWithdrawQueuedInput: (inputId) => {
      evidence.withdrawn.push(inputId)
      const item = queued.find((entry) => entry.id === inputId)
      setQueued((current) => current.filter((entry) => entry.id !== inputId))
      return Promise.resolve(item ? { text: String(item.text || ''), attachments: [] } : null)
    },
    onAbort: () => {
      evidence.aborts += 1
      return new Promise<void>((resolve) =>
        Object.assign(window, {
          settleAbort: () => {
            setStreaming(false)
            setQueued([])
            resolve()
          },
        }),
      )
    },
  }
  Object.assign(window, {
    replicaChat: {
      evidence,
      switchSession: () => setSessionId((id) => (id === 'fixture-a' ? 'fixture-b' : 'fixture-a')),
      history: () =>
        setHistory({
          ...DEFAULT_SESSION_STATE,
          messages: rows(300, 200),
          messageStart: 300,
          hasOlder: true,
          olderCursor: '300',
        }),
      older: loadOlder,
    },
  })
  return (
    <SessionPageChromeContext.Provider value={pageChrome}>
      <FocusSession {...props} />
    </SessionPageChromeContext.Provider>
  )
}

createRoot(document.getElementById('root')!).render(
  <MemoryRouter>
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>
        <div style={{ height: '100vh', width: '100%' }}>
          <Fixture />
        </div>
      </TooltipProvider>
    </QueryClientProvider>
  </MemoryRouter>,
)
