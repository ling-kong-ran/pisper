import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type FormEvent,
} from 'react'
import { ArrowUp, LoaderCircle, MessageSquare, Square } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { matchesShortcut } from '@/lib/shortcuts'
import { useShortcutStore } from '@/stores/shortcut-store'
import { ChatRequestNotice } from './ChatRequestNotice'
import { FocusTranscript } from './FocusTranscript'
import { useSideChat } from './side-chat-context'
import { ToolApproval } from './ToolApproval'
import { chatErrorMessage } from './chat-errors'
import { resolveSessionStreaming } from './session-streaming-state'

export function SideChatPane({ parentId, active }: { parentId: string; active: boolean }) {
  const { t } = useI18n()
  const { entry, runtime, load, setDraft } = useSideChat(parentId)
  const sessionId = entry.session?.id ?? ''
  const subscribe = useMemo(
    () => (listener: () => void) => runtime.subscribeSessionState(sessionId, listener),
    [runtime, sessionId],
  )
  const getSnapshot = useMemo(() => () => runtime.getSessionState(sessionId), [runtime, sessionId])
  const state = useSyncExternalStore(subscribe, getSnapshot)
  const [starting, setStarting] = useState(false)
  const [stopping, setStopping] = useState(false)
  const [actionError, setActionError] = useState('')
  const submitting = useRef(false)
  const composing = useRef(false)
  const currentDraft = useRef(entry.draft)
  currentDraft.current = entry.draft
  const inputRef = useRef<HTMLTextAreaElement>(null)
  const shortcuts = useShortcutStore((store) => store.bindings)
  const streaming = resolveSessionStreaming(state, entry.session)

  useEffect(() => {
    if (active) void load(parentId)
  }, [active, load, parentId])

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    const text = currentDraft.current.trim()
    if (!text || submitting.current || streaming || entry.expired) return
    submitting.current = true
    setStarting(true)
    setActionError('')
    try {
      // GET 不续期；存在的侧聊过期时先显示重开动作，不能悄悄把输入发进新会话。
      const response = await load(parentId, !entry.session)
      if (!response?.session) return
      const id = response.session.id
      if (runtime.getSessionState(id).streaming) return
      if (currentDraft.current.trim() === text) setDraft(parentId, '')
      setStarting(false)
      await runtime.send(id, text)
      await load(parentId)
    } catch (error) {
      setActionError(chatErrorMessage(error))
    } finally {
      submitting.current = false
      setStarting(false)
    }
  }
  const retry = useCallback(async () => {
    if (!sessionId || runtime.getSessionState(sessionId).streaming) return
    setActionError('')
    try {
      await runtime.retry(sessionId)
      await load(parentId)
    } catch (error) {
      setActionError(chatErrorMessage(error))
    }
  }, [load, parentId, runtime, sessionId])
  const stop = async () => {
    if (!sessionId || stopping) return
    setStopping(true)
    try {
      setActionError('')
      await runtime.abort(sessionId)
    } catch (error) {
      setActionError(chatErrorMessage(error))
    } finally {
      setStopping(false)
    }
  }
  const empty = (
    <div className="grid h-full min-h-36 place-content-center justify-items-center gap-3 px-3 text-center text-sm text-muted-foreground">
      <MessageSquare size={22} aria-hidden="true" />
      <p className="max-w-64 leading-relaxed">
        {entry.expired ? t('chat:sideChat.expired') : t('chat:sideChat.empty')}
      </p>
      {entry.expired && (
        <Button
          variant="outline"
          size="sm"
          disabled={entry.loading}
          onClick={async () => {
            const response = await load(parentId, true)
            if (response?.session) inputRef.current?.focus()
          }}
        >
          {entry.loading && <LoaderCircle className="size-4 animate-spin" />}
          {t('chat:sideChat.restart')}
        </Button>
      )}
    </div>
  )
  const error = actionError || entry.error || state.error
  return (
    <section
      data-side-chat-parent={parentId}
      aria-label={t('chat:sideChat.title')}
      className="flex h-full min-h-0 min-w-0 flex-1 flex-col [container-type:inline-size]"
    >
      <p className="shrink-0 border-b border-border/50 px-3 py-2 text-xs leading-relaxed text-muted-foreground">
        {t('chat:sideChat.retention')}
      </p>
      {sessionId ? (
        <FocusTranscript
          sessionId={sessionId}
          messages={state.messages}
          transcriptLoadState={state.loaded ? 'ready' : state.error ? 'error' : 'loading'}
          emptyState={empty}
          messageStart={state.messageStart}
          hasOlder={state.hasOlder}
          loadingOlder={state.loadingOlder}
          olderError={state.olderError}
          activityFeed={state.activityFeed}
          tools={state.tools}
          thinkingText={state.thinkingText}
          currentActivity={state.currentActivity}
          compaction={state.compaction}
          streaming={streaming}
          runStartedAt={state.runStartedAt}
          lastActivityAt={state.lastActivityAt}
          runFinishedAt={state.runFinishedAt}
          runStopped={state.runStopped}
          runNotice={state.runNotice}
          error={state.error}
          cwd={entry.session?.cwd}
          onLoadOlder={() => runtime.loadOlderMessages(sessionId)}
          onRetryLastTurn={retry}
        />
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">{empty}</div>
      )}
      <form
        className="flex shrink-0 flex-col gap-2 border-t border-border/50 p-3"
        onSubmit={submit}
      >
        {error && (
          <ChatRequestNotice
            error={error}
            onRetry={
              entry.error || !state.loaded
                ? async () => {
                    await load(parentId)
                  }
                : retry
            }
          />
        )}
        <ToolApproval
          approvals={state.approvals}
          onResolve={async (id, approved) => {
            try {
              await runtime.approve(sessionId, id, approved)
            } catch {
              /* 请求状态由会话控制器展示。 */
            }
          }}
        />
        <div className="flex min-w-0 items-end gap-2 rounded-xl border border-border/70 bg-muted/40 p-2 focus-within:border-ring/50">
          <textarea
            ref={inputRef}
            rows={2}
            value={entry.draft}
            aria-label={t('chat:sideChat.message')}
            placeholder={t('chat:sideChat.placeholder')}
            className="max-h-40 min-h-14 min-w-0 flex-1 resize-y border-0 bg-transparent px-1 py-1 text-sm leading-relaxed text-foreground outline-none placeholder:text-muted-foreground"
            onChange={(event) => setDraft(parentId, event.target.value)}
            onCompositionStart={() => {
              composing.current = true
            }}
            onCompositionEnd={() => {
              composing.current = false
            }}
            onKeyDown={(event) => {
              if (
                !composing.current &&
                !event.nativeEvent.isComposing &&
                event.nativeEvent.keyCode !== 229 &&
                matchesShortcut(event.nativeEvent, shortcuts.sendMessage)
              ) {
                event.preventDefault()
                event.currentTarget.form?.requestSubmit()
              }
            }}
          />
          <Button
            type={streaming ? 'button' : 'submit'}
            size="icon"
            className="size-9 shrink-0 rounded-full"
            aria-label={streaming ? t('chat:sideChat.stop') : t('chat:sideChat.send')}
            disabled={
              streaming
                ? stopping
                : starting || entry.loading || entry.expired || !entry.draft.trim()
            }
            onClick={streaming ? () => void stop() : undefined}
          >
            {starting || stopping ? (
              <LoaderCircle className="size-4 animate-spin" />
            ) : streaming ? (
              <Square size={13} fill="currentColor" />
            ) : (
              <ArrowUp size={17} />
            )}
          </Button>
        </div>
      </form>
    </section>
  )
}
