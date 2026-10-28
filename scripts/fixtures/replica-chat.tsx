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
import { DEFAULT_SESSION_STATE } from '@/lib/session-state'
import type { ChatMessage } from '@/types/chat'
import '@/index.css'

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
  { id: 'agent', role: 'agent', text: markdown },
]
const rows = (start: number, count: number): ChatMessage[] =>
  Array.from({ length: count }, (_, i) => ({
    id: `m${start + i}`,
    role: i % 2 ? 'agent' : 'user',
    text: `Message ${start + i}\n\n` + '固定阅读锚点。Stable reading anchor. '.repeat(3),
  }))
const evidence = { sends: 0, queues: 0, aborts: 0, attachments: 0 }
const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })

export function Fixture() {
  const [sessionId, setSessionId] = useState('fixture-a')
  const [streaming, setStreaming] = useState(false)
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
    tools: [],
    approvals: [],
    activityFeed: [],
    queuedInputs: [],
    streaming,
    contextOpen: false,
    contextCompact: false,
    contextPanelId: 'context',
    onToggleContext: noop,
    onModelChange: noop,
    onThinkingLevelChange: noop,
    onExecutionModeChange: () => true,
    onRunModeChange: () => true,
    onApproval: noop,
    onWorkspace: noop,
    onRename: noop,
    onBranchFromHere: noop,
    onCreateChildSession: noop,
    onRetryLastTurn: noop,
    onSplitLeft: noop,
    onSplitRight: noop,
    onSplitTop: noop,
    onSplitBottom: noop,
    onClosePanel: noop,
    onSend: (_text, attachments, _goal, _team, _budget, _invocation, onAccepted) => {
      evidence.sends += 1
      evidence.attachments = attachments.length
      return new Promise<boolean>((resolve) =>
        Object.assign(window, {
          settleSend: (accepted: boolean) => {
            if (accepted) {
              onAccepted?.()
              setStreaming(true)
            }
            resolve(accepted)
          },
        }),
      )
    },
    onQueue: () => {
      evidence.queues += 1
      return Promise.resolve(true)
    },
    onAbort: () => {
      evidence.aborts += 1
      return new Promise<void>((resolve) =>
        Object.assign(window, {
          settleAbort: () => {
            setStreaming(false)
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
  return <FocusSession {...props} />
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
