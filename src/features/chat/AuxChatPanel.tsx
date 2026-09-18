// 辅助对话面板（右栏）：浏览器多标签式快捷问答栏。
// 顶部标签条与 dockview 标签同款视觉（标签×、右侧＋新建）；每个标签对应一个
// 独立会话，新建时沿用当前主会话的工作目录（同一个项目）；列表与激活项持久化。
// 复用会话 HTTP/SSE API，与主对话互不干扰。
import { useCallback, useEffect, useRef, useState, type PointerEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { FolderOpen, Loader2, Pencil, Plus, TreePine, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { fetchStartupQuery, startupQueryOptions } from '@/lib/startup-queries'
import { chatApi } from './chat-api'
import { ComposerSendButton } from './focus-session-composer-bits'
import { SessionTreeDialog } from './SessionTreeDialog'
import { WorkspacePicker } from '@/components/WorkspacePicker'

const AUX_SESSIONS_KEY = 'pisper-aux-sessions'
const AUX_ACTIVE_KEY = 'pisper-aux-active'
const AUX_WIDTH_KEY = 'pisper-aux-width'

type AuxMessage = { role: 'user' | 'agent'; text: string }

function readAuxIds(): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(AUX_SESSIONS_KEY) || '[]')
    return Array.isArray(raw) ? raw.filter((id) => typeof id === 'string') : []
  } catch {
    return []
  }
}

function persistAuxIds(ids: string[]) {
  localStorage.setItem(AUX_SESSIONS_KEY, JSON.stringify(ids))
}

export function AuxChatPanel({ cwd, onClose }: { cwd?: string; onClose: () => void }) {
  const { t } = useI18n()
  const [auxIds, setAuxIds] = useState<string[]>(readAuxIds)
  const [activeId, setActiveId] = useState(
    () => localStorage.getItem(AUX_ACTIVE_KEY) || readAuxIds()[0] || '',
  )
  const [messages, setMessages] = useState<AuxMessage[]>([])
  const [value, setValue] = useState('')
  const [streaming, setStreaming] = useState(false)
  const [error, setError] = useState('')
  const listRef = useRef<HTMLDivElement>(null)
  const activeIdRef = useRef(activeId)
  activeIdRef.current = activeId
  const streamingRef = useRef(false)
  streamingRef.current = streaming
  // ⋯ 菜单联动：追忆树开合、工作目录选择弹窗。
  const [treeOpen, setTreeOpen] = useState(false)
  const [workspacePickerOpen, setWorkspacePickerOpen] = useState(false)

  // 重命名当前辅助会话标签（⋯ 菜单 → 重命名）：PATCH 成功后刷新摘要。
  const renameActive = async () => {
    if (!activeId) return
    const currentName = titleOf(activeId, auxIds.indexOf(activeId))
    const name = window.prompt(t('chat:chatHistoryPage.renameChat'), currentName)
    if (name === null || name === currentName) return
    try {
      await apiJson(`/api/sessions/${encodeURIComponent(activeId)}`, {
        method: 'PATCH',
        body: JSON.stringify({ name }),
      })
      fetchStartupQuery('sessions', true).catch(() => {})
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  // 工作目录切换（⋯ 菜单 → 设置工作目录）：目录选择弹窗复用 WorkspacePicker。
  const switchWorkspace = async (path: string) => {
    if (!activeId) return
    try {
      await apiJson(`/api/sessions/${encodeURIComponent(activeId)}`, {
        method: 'PATCH',
        body: JSON.stringify({ cwd: path }),
      })
      fetchStartupQuery('sessions', true).catch(() => {})
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  // 标签标题：直接复用启动查询的会话摘要（自动命名会随首条消息更新）。
  const { data: sessionsData } = useQuery({
    ...startupQueryOptions<{ sessions: Array<{ id: string; name?: string }> }>('sessions'),
    refetchInterval: 20_000,
  })
  const titleOf = (id: string, index: number) =>
    sessionsData?.sessions?.find((session) => session.id === id)?.name ||
    `${t('chat:auxChat.defaultName')} ${index + 1}`

  // 新建辅助对话：沿用当前主会话的项目目录（同一个项目下追加对话）。
  const createAux = useCallback(async () => {
    try {
      const created = await apiJson<{ id: string }>('/api/sessions', {
        method: 'POST',
        body: JSON.stringify({
          name: t('chat:auxChat.defaultName'),
          ...(cwd ? { cwd } : {}),
        }),
      })
      setAuxIds((current) => {
        const next = [...current, created.id]
        persistAuxIds(next)
        return next
      })
      setActiveId(created.id)
      localStorage.setItem(AUX_ACTIVE_KEY, created.id)
      setMessages([])
      setError('')
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }, [cwd, t])

  // 挂载时保证至少存在一个辅助会话（首次使用或全部被关后重建）。
  useEffect(() => {
    if (readAuxIds().length) return
    void createAux()
    // 仅挂载时执行；cwd 后续变化只影响「再新建」的目录。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // 切换标签：加载该会话最近消息（失败静默，保留空态提示）。
  useEffect(() => {
    if (!activeId) return
    let cancelled = false
    setMessages([])
    chatApi
      .getMessages(activeId, { limit: 30 })
      .then((page) => {
        if (cancelled) return
        setMessages(
          (page.messages || [])
            .filter((m) => m.role === 'user' || m.role === 'agent')
            .map((m) => ({
              role: m.role === 'user' ? 'user' : 'agent',
              text: typeof m.text === 'string' ? m.text : '',
            })),
        )
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [activeId])

  // 新消息到达时滚到底部。
  useEffect(() => {
    const list = listRef.current
    if (list) list.scrollTop = list.scrollHeight
  }, [messages])

  const send = useCallback(async () => {
    const text = value.trim()
    if (!text || !activeId || streamingRef.current) return
    setValue('')
    setError('')
    setStreaming(true)
    setMessages((current) => [...current, { role: 'user', text }, { role: 'agent', text: '' }])
    const targetId = activeId
    let agentIndex = -1
    const onEvent = (event: string, data: Record<string, unknown>) => {
      if (event === 'text_delta') {
        const delta = String(data?.delta || '')
        // 切换标签后旧流的增量不再写入当前缓冲。
        if (activeIdRef.current !== targetId) return
        setMessages((current) => {
          if (agentIndex < 0) agentIndex = current.length - 1
          const next = [...current]
          next[agentIndex] = { role: 'agent', text: (next[agentIndex]?.text || '') + delta }
          return next
        })
      } else if (event === 'run_failed' || event === 'error') {
        const message =
          String(data?.error || data?.message || '') || t('chat:auxChat.requestFailed')
        setError(message)
      }
    }
    try {
      await chatApi.openStream(
        {
          sessionId: targetId,
          message: text,
          attachments: [],
          goalMode: false,
          teamMode: false,
          goalTokenBudget: null,
        },
        onEvent,
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setStreaming(false)
    }
  }, [activeId, t, value])

  // 关闭标签：关到最后一个时连面板一起收起。
  const closeTab = (id: string) => {
    const remaining = auxIds.filter((item) => item !== id)
    persistAuxIds(remaining)
    setAuxIds(remaining)
    if (!remaining.length) {
      localStorage.removeItem(AUX_ACTIVE_KEY)
      onClose()
      return
    }
    if (activeId === id) {
      const next = remaining[remaining.length - 1]
      setActiveId(next)
      localStorage.setItem(AUX_ACTIVE_KEY, next)
    }
  }

  const widthDrag = useRef<{ startX: number; startWidth: number } | null>(null)
  const [width, setWidth] = useState(() => Number(localStorage.getItem(AUX_WIDTH_KEY)) || 360)
  const widthRef = useRef(width)
  widthRef.current = width

  const startDrag = (event: PointerEvent<HTMLDivElement>) => {
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    widthDrag.current = { startX: event.clientX, startWidth: width }
  }
  const moveDrag = (event: PointerEvent<HTMLDivElement>) => {
    if (!widthDrag.current) return
    // 向左拖 = 变宽：clientX 减小、宽度增大。
    const next = Math.min(
      560,
      Math.max(260, widthDrag.current.startWidth + (widthDrag.current.startX - event.clientX)),
    )
    setWidth(next)
  }
  const endDrag = () => {
    widthDrag.current = null
    localStorage.setItem(AUX_WIDTH_KEY, String(widthRef.current))
  }

  return (
    <div
      className="aux-chat-panel relative flex h-full min-h-0 flex-col overflow-hidden bg-[var(--main-surface-bg)]"
      style={{ width }}
    >
      {/* 浏览器多标签式标签条：与中栏 dockview 标签同款视觉。 */}
      <div className="flex h-[36px] flex-none items-stretch gap-[2px] [border-bottom:1px_solid_var(--stroke-soft)] bg-[var(--surface-subtle)] [padding:0_6px]">
        <div className="flex min-w-0 flex-1 items-stretch gap-[2px] overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {auxIds.map((id, index) => (
            <div
              className={`aux-tab group/tab flex min-w-0 max-w-[160px] flex-none cursor-pointer items-center gap-[6px] [padding:0_4px_0_10px] text-[12px] [transition:var(--d1)_var(--ease-out)] ${id === activeId ? 'bg-[var(--panel)] font-[600] text-[var(--text)] shadow-[inset_0_-2px_var(--brand-blue)]' : 'font-[500] text-[var(--text-muted)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-soft)]'}`}
              key={id}
              onClick={() => {
                setActiveId(id)
                localStorage.setItem(AUX_ACTIVE_KEY, id)
              }}
              role="tab"
              aria-selected={id === activeId}
            >
              <span className="min-w-0 flex-1 truncate">{titleOf(id, index)}</span>
              <button
                type="button"
                className="grid h-[18px] w-[18px] flex-none place-items-center border-0 rounded-[var(--r-xs)] bg-transparent text-[var(--text-muted)] opacity-0 cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--text)] group-hover/tab:opacity-100"
                title={t('chat:auxChat.closeTab')}
                aria-label={t('chat:auxChat.closeTab')}
                onClick={(event) => {
                  event.stopPropagation()
                  closeTab(id)
                }}
              >
                <X size={12} />
              </button>
            </div>
          ))}
        </div>
        <button
          type="button"
          className="grid w-[28px] flex-none place-items-center border-0 bg-transparent text-[var(--text-muted)] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--text)]"
          title={t('chat:auxChat.newTab')}
          aria-label={t('chat:auxChat.newTab')}
          onClick={() => void createAux()}
        >
          <Plus size={15} />
        </button>
      </div>
      <div
        ref={listRef}
        className="flex min-h-0 flex-1 flex-col gap-[10px] overflow-y-auto [padding:12px_12px_4px]"
      >
        {!messages.length && (
          <p className="m-0 text-[var(--app-message-font-size)] leading-[1.6] text-[var(--text-muted)]">
            {t('chat:auxChat.emptyHint')}
          </p>
        )}
        {messages.map((message, index) => (
          <div
            className={
              message.role === 'user'
                ? 'self-end max-w-[86%] rounded-[var(--r-md)] bg-[var(--user-bubble-bg)] [padding:7px_10px] text-[var(--app-message-font-size)] leading-[1.6] text-[var(--user-bubble-text)] whitespace-pre-wrap break-words'
                : 'self-start max-w-[92%] text-[var(--app-message-font-size)] leading-[1.7] text-[var(--text-soft)] whitespace-pre-wrap break-words'
            }
            key={index}
          >
            {message.text ||
              (message.role === 'agent' && streaming && index === messages.length - 1 ? (
                <Loader2 size={13} className="animate-spin text-[var(--text-muted)]" />
              ) : (
                ''
              ))}
          </div>
        ))}
        {error && (
          <p className="m-0 rounded-[var(--r-sm)] bg-[var(--danger-soft)] [padding:7px_9px] text-[11.5px] text-[var(--danger)]">
            {error}
          </p>
        )}
      </div>
      {/* 输入区与中栏主对话框逐字节同款：同容器类（含 solid 底与投影）、
          同 textarea 规格、同一个 ComposerSendButton 组件。 */}
      <form
        className="aux-composer-shell flex-none [padding:10px_12px_10px]"
        onSubmit={(event) => {
          event.preventDefault()
          void send()
        }}
      >
        <div className="focus-composer relative flex min-w-0 flex-col items-stretch gap-[4px] [border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--solid)] [padding:8px] shadow-[0_14px_34px_-24px_var(--shadow-strong)] [transition:border-color_var(--d1)_var(--ease-out),_box-shadow_var(--d2)_var(--ease-out)] [&:focus-within]:border-[var(--focus)] [&:focus-within]:shadow-[0_0_0_3px_var(--focus-ring)] [&_textarea]:[outline:0]!">
          <textarea
            className="w-full min-w-0 min-h-[48px] max-h-[220px] [align-self:start] resize-none overflow-y-auto border-0 [outline:0] bg-transparent p-[5px_6px_8px] text-[var(--text)] text-[14px] leading-[1.5] placeholder:text-[var(--text-muted)]"
            placeholder={t('chat:auxChat.placeholder')}
            aria-label={t('chat:auxChat.title')}
            rows={1}
            value={value}
            onChange={(event) => {
              setValue(event.target.value)
              event.currentTarget.style.height = 'auto'
              event.currentTarget.style.height = `${Math.min(event.currentTarget.scrollHeight, 220)}px`
            }}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault()
                void send()
              }
            }}
          />
          <div className="focus-composer-footer flex min-w-0 items-center gap-[2px]">
            {/* 常用操作静态平铺：工作目录 / 追忆 / 重命名 / 关闭面板，无折叠菜单。 */}
            <button
              type="button"
              className="grid h-[34px] w-[34px] flex-none place-items-center border-0 rounded-[var(--r-sm)] bg-transparent text-[var(--text-muted)] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--text)]"
              title={t('chat:focusSession.setWorkingDirectory')}
              aria-label={t('chat:focusSession.setWorkingDirectory')}
              onClick={() => setWorkspacePickerOpen(true)}
            >
              <FolderOpen size={16} />
            </button>
            <button
              type="button"
              className="grid h-[34px] w-[34px] flex-none place-items-center border-0 rounded-[var(--r-sm)] bg-transparent text-[var(--text-muted)] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--text)]"
              title={t('chat:sessionTree.menu')}
              aria-label={t('chat:sessionTree.menu')}
              onClick={() => setTreeOpen(true)}
            >
              <TreePine size={16} />
            </button>
            <button
              type="button"
              className="grid h-[34px] w-[34px] flex-none place-items-center border-0 rounded-[var(--r-sm)] bg-transparent text-[var(--text-muted)] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--text)]"
              title={t('chat:focusSession.renameChat')}
              aria-label={t('chat:focusSession.renameChat')}
              onClick={() => void renameActive()}
            >
              <Pencil size={16} />
            </button>
            <button
              type="button"
              className="grid h-[34px] w-[34px] flex-none place-items-center border-0 rounded-[var(--r-sm)] bg-transparent text-[var(--text-muted)] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--text)]"
              title={t('chat:focusSession.closeTab')}
              aria-label={t('chat:focusSession.closeTab')}
              onClick={onClose}
            >
              <X size={16} />
            </button>
            <div className="flex-1" />
            <ComposerSendButton
              streaming={streaming}
              queueing={false}
              disabled={!streaming && (!value.trim() || !activeId)}
              onAbort={() => {
                void chatApi.abort(activeId).catch(() => {})
              }}
            />
          </div>
        </div>
      </form>
      {/* 拖宽手柄：贴左缘 */}
      <div
        className="absolute top-0 bottom-0 left-0 z-[5] w-[5px] cursor-col-resize after:absolute after:inset-y-0 after:left-[2px] after:w-px hover:after:bg-[var(--stroke-hover)]"
        onPointerDown={startDrag}
        onPointerMove={moveDrag}
        onPointerUp={endDrag}
        aria-hidden="true"
      />
      {treeOpen && (
        <SessionTreeDialog
          open
          sessionId={activeId}
          streaming={streaming}
          onClose={() => setTreeOpen(false)}
          onNavigated={() => {}}
          onCreateChildSession={() => {}}
        />
      )}
      {workspacePickerOpen && (
        <WorkspacePicker
          open
          initialPath={cwd}
          onOpenChange={setWorkspacePickerOpen}
          onSelect={(path) => void switchWorkspace(path)}
        />
      )}
    </div>
  )
}
