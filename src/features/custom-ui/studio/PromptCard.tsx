// 工作台的提示词卡片（规格 5.1 / 5.2）：“用一句话描述 · 代理会生成或修改这个组件”，输入区 + [生成 ⌘↵]。
// 生成走这个组件的绑定会话（use-component-agent），会话出现在侧栏“运行中”；结果行显示该会话在看板里的状态
// 与 [打开会话]，状态用中性色（琥珀只表示需要你）。A8 补上“基于 vN”芯片与结果行“vN 已生成 · 写入 … · 由代理生成”
// 及改动行数芯片（数据来自版本清单：代理每次写入都会留一份快照与改动摘要）。模型芯片不做：绑定会话沿用
// 会话自己的模型，要换模型在打开的会话里换（替负责人做的决定，见 decisions.md）。
// 草稿按组件记在本页内存里：切到源码 / 日志再回来、或热重载重挂时不丢。
import { useId, useState, type KeyboardEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'
import { ArrowUpRight } from 'lucide-react'
import { sessionPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Chip } from '@/components/ui/chip'
import { Kbd } from '@/components/ui/kbd'
import { Spinner } from '@/components/ui/spinner'
import { Textarea } from '@/components/ui/textarea'
import { relativeTime } from '@/lib/format'
import { formatShortcut } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { useBoardSessions } from '@/features/board/public'
import type { ComponentRow } from '@/features/custom-ui/custom-ui-api'
import { componentCatalogQueryOptions } from '@/features/custom-ui/queries'
import {
  AGENT_PROMPT_MAX,
  useComponentAgent,
  useComponentAgentState,
  type ComponentAgentResult,
} from './use-component-agent'
import { useComponentVersions } from './use-lifecycle'
import { VersionFileChips } from './VersionsSection'

const drafts = new Map<string, string>()

// 结果行：这次交给代理的会话在看板里的状态（运行中 / 需要你 / 排队 / 已完成）。
function AgentSessionStatus({ sessionId, onOpen }: { sessionId: string; onOpen: () => void }) {
  const { t } = useI18n()
  const board = useBoardSessions({ doneWindow: 'today' })
  const lane = board.all.find((item) => item.id === sessionId)?.lane
  const label =
    lane === 'running'
      ? t('custom-ui:agent.statusRunning')
      : lane === 'attention'
        ? t('custom-ui:agent.statusAttention')
        : lane === 'queued'
          ? t('custom-ui:agent.statusQueued')
          : lane === 'done'
            ? t('custom-ui:agent.statusDone')
            : t('custom-ui:agent.statusSent')
  return (
    <span
      data-component-agent-status={lane ?? 'sent'}
      className="flex min-w-0 items-center gap-1.5 text-xs text-content-muted"
    >
      <span
        aria-hidden
        className={cn(
          'size-1.5 flex-none rounded-full',
          lane === 'attention' ? 'bg-st-attn' : lane === 'running' ? 'bg-st-run' : 'bg-st-done',
        )}
      />
      <span className="truncate">{label}</span>
      <Button
        size="xs"
        variant="ghost"
        data-component-agent-open={sessionId}
        className="flex-none"
        onClick={onOpen}
      >
        {t('custom-ui:agent.openSession')}
        <ArrowUpRight />
      </Button>
    </span>
  )
}

export function PromptCard({
  row,
  notify,
  autoFocus = false,
  onSent,
  className,
}: {
  row: ComponentRow
  notify: (message: string) => void
  autoFocus?: boolean
  onSent?: (result: ComponentAgentResult) => void
  className?: string
}) {
  const { t, language } = useI18n()
  const navigate = useNavigate()
  const id = useId()
  const catalog = useQuery(componentCatalogQueryOptions())
  const versions = useComponentVersions(row.id, catalog.data?.features.versions === true)
  const current = versions.data?.current
  // 结果行只描述代理写出来的那一版；导入、回滚或手改的版本不在提示词卡片里说。
  const generated = current?.source === 'agent' ? current : null
  const agent = useComponentAgent(notify)
  const { pending, sessionId } = useComponentAgentState(row.id)
  const [draft, setDraftState] = useState(() => drafts.get(row.id) ?? '')
  const setDraft = (value: string) => {
    if (value) drafts.set(row.id, value)
    else drafts.delete(row.id)
    setDraftState(value)
  }
  const trimmed = draft.trim()
  const submit = async () => {
    if (!trimmed || pending) return
    const result = await agent.generate(row, trimmed)
    if (!result) return
    setDraft('')
    onSent?.(result)
  }
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== 'Enter' || !(event.metaKey || event.ctrlKey) || event.nativeEvent.isComposing)
      return
    event.preventDefault()
    void submit()
  }
  return (
    <form
      data-component-prompt={row.id}
      className={cn(
        'flex flex-col gap-2 rounded-[var(--r-lg)] border border-stroke bg-panel p-3',
        className,
      )}
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
    >
      <span className="flex min-w-0 items-center gap-2">
        <label htmlFor={id} className="mr-auto text-xs font-medium text-content-secondary">
          {t('custom-ui:agent.promptTitle')}
        </label>
        {current && (
          // 只是说明（生成会改哪一版），不是可点的选择键：渲染成 span，不进 Tab 顺序。
          <Chip asChild size="sm" className="hover:bg-solid hover:text-content-secondary">
            <span data-prompt-base={current.n}>
              {t('custom-ui:agent.basedOn', { n: current.n })}
            </span>
          </Chip>
        )}
      </span>
      <Textarea
        id={id}
        autoFocus={autoFocus}
        rows={2}
        value={draft}
        maxLength={AGENT_PROMPT_MAX}
        placeholder={t('custom-ui:agent.promptPlaceholder')}
        aria-keyshortcuts="Meta+Enter Control+Enter"
        data-component-prompt-input
        className="max-h-40 min-h-14 resize-none"
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={onKeyDown}
      />
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {sessionId ? (
          <AgentSessionStatus
            sessionId={sessionId}
            onOpen={() => navigate(sessionPath(sessionId))}
          />
        ) : (
          <span className="text-xs text-content-muted">{t('custom-ui:agent.promptHint')}</span>
        )}
        <Button
          type="submit"
          size="sm"
          disabled={!trimmed || pending}
          aria-keyshortcuts="Meta+Enter Control+Enter"
          data-component-generate
          className="ml-auto"
        >
          {pending && <Spinner />}
          {pending ? t('custom-ui:agent.sending') : t('custom-ui:agent.generate')}
          {/* 与侧栏、快捷键设置同一个格式化：macOS 显示 Cmd，其余平台显示 Ctrl。 */}
          {!pending && (
            <Kbd className="bg-transparent text-current">{formatShortcut('Mod+Enter')}</Kbd>
          )}
        </Button>
      </div>
      {generated && (
        <div data-prompt-result={generated.n} className="flex min-w-0 flex-col gap-1">
          <p className="truncate text-xs text-content-muted">
            {[
              t('custom-ui:agent.generatedVersion', { n: generated.n }),
              generated.files.length
                ? t('custom-ui:agent.wrote', {
                    files: generated.files.map((file) => file.path).join(' '),
                  })
                : '',
              t('custom-ui:versions.sourceAgent'),
              generated.at ? relativeTime(generated.at, language) : '',
            ]
              .filter(Boolean)
              .join(' · ')}
          </p>
          <VersionFileChips version={generated} />
        </div>
      )}
    </form>
  )
}
