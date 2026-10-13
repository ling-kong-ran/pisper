// 会话树详情：突出所选消息与主要操作，标签和谱系保留在同一面板。
import type { RefObject } from 'react'
import {
  ChevronLeft,
  GitBranch,
  MessageSquare,
  MessageSquarePlus,
  RefreshCw,
  Tag,
  Trash2,
} from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'
import type { SessionSummary } from '@/types/chat'
import type { SessionTreeNode } from '@/features/chat/api/chat-api'
import type { DisplayNode } from '@/features/chat/model/session-tree-model'

type NodeLabelFn = (node: SessionTreeNode) => string

export function SessionTreeInspector({
  mobileLayout,
  selected,
  loading,
  error,
  streaming,
  dataStreaming,
  parentSession,
  parentEntryId,
  childSessions,
  canLabel,
  canDerive,
  canDelete,
  busy,
  deleting,
  deleteButtonRef,
  label,
  summarize,
  savingLabel,
  creatingChild,
  navigating,
  openingRelatedId,
  typeLabel,
  stateLabel,
  onBackToList,
  onLabelChange,
  onSummarizeChange,
  onSaveLabel,
  onRemoveLabel,
  onCreateChild,
  onNavigate,
  onRefresh,
  onDeleteBranch,
  onOpenRelated,
}: {
  mobileLayout: boolean
  selected: DisplayNode | null
  loading: boolean
  error: string
  streaming: boolean
  dataStreaming: boolean
  parentSession: SessionSummary | null
  parentEntryId: string
  childSessions: SessionSummary[]
  canLabel: boolean
  canDerive: boolean
  canDelete: boolean
  busy: boolean
  deleting: boolean
  deleteButtonRef: RefObject<HTMLButtonElement | null>
  label: string
  summarize: boolean
  savingLabel: boolean
  creatingChild: boolean
  navigating: boolean
  openingRelatedId: string
  typeLabel: NodeLabelFn
  stateLabel: NodeLabelFn
  onBackToList: () => void
  onLabelChange: (value: string) => void
  onSummarizeChange: (value: boolean) => void
  onSaveLabel: () => void
  onRemoveLabel: () => void
  onCreateChild: () => void
  onNavigate: () => void
  onRefresh: () => void
  onDeleteBranch: () => void
  onOpenRelated: (target: SessionSummary, targetEntryId?: string) => void
}) {
  const { t, language } = useI18n()
  const disabled = busy || streaming || dataStreaming
  const timestamp = Date.parse(selected?.timestamp || '')

  return (
    <div
      className={cn(
        'chat-resource-config session-tree-inspector flex min-h-0 min-w-0 flex-col gap-4 overflow-y-auto overscroll-contain p-[18px]',
        mobileLayout
          ? 'bg-[var(--solid)]'
          : 'm-[12px_12px_12px_0] rounded-[var(--r-md)] border border-[var(--stroke-soft)] bg-[var(--solid)]',
      )}
    >
      {mobileLayout && (
        <Button variant="ghost" className="min-h-10 self-start" onClick={onBackToList}>
          <ChevronLeft size={17} />
          {t('chat:sessionTree.backToNodes')}
        </Button>
      )}
      {error && (
        <div role="alert" className="rounded-lg border border-danger-border bg-danger-soft p-3">
          <p className="text-xs break-words text-danger">{error}</p>
          <Button variant="ghost" size="sm" disabled={busy} onClick={onRefresh}>
            <RefreshCw size={13} />
            {t('chat:sessionTree.retry')}
          </Button>
        </div>
      )}
      {selected ? (
        <>
          <div className="min-w-0 space-y-2">
            <div className="flex items-center justify-between gap-2 text-xs text-[var(--text-muted)]">
              <span>{t('chat:sessionTree.selectedNode')}</span>
              {stateLabel(selected) && (
                <span className="rounded-md bg-[var(--brand-blue-soft)] px-2 py-1 text-[10px] text-[var(--brand-blue-strong)]">
                  {stateLabel(selected)}
                </span>
              )}
            </div>
            <h3 className="text-sm font-semibold break-words">
              {selected.label || typeLabel(selected)}
            </h3>
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px] text-[var(--text-muted)]">
              <span>{typeLabel(selected)}</span>
              {Number.isFinite(timestamp) && (
                <time dateTime={selected.timestamp}>
                  {new Intl.DateTimeFormat(language, {
                    dateStyle: 'medium',
                    timeStyle: 'short',
                  }).format(timestamp)}
                </time>
              )}
            </div>
            <p className="max-h-[240px] overflow-y-auto overscroll-contain rounded-lg bg-[var(--surface-subtle)] p-3 text-[13px] leading-relaxed whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
              {selected.text || t('chat:sessionTree.noPreview')}
            </p>
          </div>
          {canLabel && (
            <div className="space-y-2 border-t border-[var(--stroke-soft)] pt-4">
              <label className="grid gap-2 text-xs font-medium">
                <span>{t('chat:sessionTree.nodeLabel')}</span>
                <Input
                  value={label}
                  maxLength={80}
                  disabled={disabled}
                  placeholder={t('chat:sessionTree.labelPlaceholder')}
                  onChange={(event) => onLabelChange(event.target.value)}
                />
              </label>
              <div className="flex gap-2">
                <Button
                  variant="outline"
                  className="flex-1"
                  disabled={disabled}
                  onClick={onSaveLabel}
                >
                  <Tag size={14} />
                  {savingLabel
                    ? t('chat:sessionTree.savingLabel')
                    : t('chat:sessionTree.saveLabel')}
                </Button>
                {Boolean(selected.label) && (
                  <Button
                    variant="ghost"
                    size="icon"
                    disabled={disabled}
                    aria-label={t('chat:sessionTree.removeLabel')}
                    title={t('chat:sessionTree.removeLabel')}
                    onClick={onRemoveLabel}
                  >
                    <Trash2 size={14} />
                  </Button>
                )}
              </div>
            </div>
          )}
          {(canDerive || canLabel) && (
            <div className="space-y-3 border-t border-[var(--stroke-soft)] pt-4">
              {canDerive && (
                <>
                  <Button
                    className="chat-resource-confirm min-h-9 w-full bg-[var(--star)] font-semibold text-[var(--on-accent)] hover:bg-[var(--star-hover)]"
                    disabled={disabled || selected.leaf}
                    onClick={onNavigate}
                  >
                    <GitBranch size={15} />
                    {navigating
                      ? t('chat:sessionTree.navigating')
                      : selected.leaf
                        ? t('chat:sessionTree.currentPosition')
                        : t('chat:sessionTree.continueFromNode')}
                  </Button>
                  <label className="grid grid-cols-[16px_minmax(0,1fr)] items-start gap-2 text-xs">
                    <input
                      className="mt-0.5 size-4 accent-[var(--blue)]"
                      type="checkbox"
                      checked={summarize}
                      disabled={disabled || selected.leaf}
                      onChange={(event) => onSummarizeChange(event.target.checked)}
                    />
                    <span>
                      {t('chat:sessionTree.abandonedBranchSummary')}
                      <small className="mt-1 block leading-relaxed text-[var(--text-muted)]">
                        {t('chat:sessionTree.abandonedBranchSummaryHint')}
                      </small>
                    </span>
                  </label>
                  <p className="text-[11px] leading-relaxed text-[var(--text-muted)]">
                    {t('chat:sessionTree.sideEffectsRemain')}
                  </p>
                </>
              )}
              {canLabel && (
                <Button
                  variant="outline"
                  className="w-full"
                  disabled={disabled}
                  onClick={onCreateChild}
                >
                  <MessageSquarePlus size={14} />
                  {creatingChild
                    ? t('chat:sessionTree.creatingChildChat')
                    : t('chat:sessionTree.createChildChat')}
                </Button>
              )}
            </div>
          )}
          {(parentSession || childSessions.length > 0) && (
            <div className="session-tree-lineage flex flex-col gap-2 border-t border-[var(--stroke-soft)] pt-4">
              <strong className="text-xs font-medium">
                {t('chat:sessionTree.conversationLineage')}
              </strong>
              {parentSession && (
                <Button
                  variant="outline"
                  className="h-auto min-h-8 justify-start text-left text-xs whitespace-normal"
                  disabled={busy || Boolean(openingRelatedId)}
                  onClick={() => onOpenRelated(parentSession, parentEntryId)}
                >
                  <MessageSquare size={13} />
                  {t('chat:sessionTree.returnToOriginalChat', {
                    name: parentSession.name || t('chat:chatPage.newChat'),
                  })}
                </Button>
              )}
              {childSessions.map((child) => (
                <Button
                  variant="outline"
                  className="h-auto min-h-8 justify-start text-left text-xs whitespace-normal"
                  disabled={busy || Boolean(openingRelatedId)}
                  key={child.id}
                  onClick={() => onOpenRelated(child)}
                >
                  <MessageSquare size={13} />
                  {t('chat:sessionTree.openSeparateChat', {
                    name: child.name || t('chat:chatPage.newChat'),
                  })}
                </Button>
              ))}
            </div>
          )}
          {canDelete && (
            <div className="mt-auto border-t border-[var(--stroke-soft)] pt-4">
              <Button
                variant="destructive"
                className="w-full"
                disabled={disabled}
                onClick={onDeleteBranch}
                ref={deleteButtonRef}
              >
                <Trash2 size={14} />
                {deleting
                  ? t('chat:sessionTree.deletingBranch')
                  : t('chat:sessionTree.deleteBranch')}
              </Button>
            </div>
          )}
        </>
      ) : (
        <p className="m-auto text-xs text-[var(--text-muted)]">
          {loading ? t('chat:sessionTree.loading') : t('chat:sessionTree.chooseNode')}
        </p>
      )}
    </div>
  )
}
