// 会话树右侧详情面板：选中节点预览、谱系跳转、标签编辑、派生对话与导航操作。
// 从 SessionTreeDialog 拆出，文案复用既有 i18n key，行为与原实现一致。
import { ChevronLeft, GitBranch, MessageSquare, MessageSquarePlus, Tag, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import type { SessionSummary } from '@/types/chat'
import type { SessionTreeNode } from '@/features/chat/chat-api'
import type { DisplayNode } from '@/features/chat/session-tree-model'

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
  onOpenRelated: (target: SessionSummary, targetEntryId?: string) => void
}) {
  const { t, language } = useI18n()

  return (
    <div
      className={`chat-resource-config session-tree-inspector flex min-h-0 min-w-0 flex-col gap-4 overflow-y-auto overscroll-contain bg-panel p-[18px] [&_>_p]:m-auto [&_>_p]:text-xs [&_>_p]:text-content-muted [&_p]:text-xs [&_p]:text-content-muted [&_strong]:text-[15px] [&_strong]:font-medium ${mobileLayout ? '' : 'border-l border-stroke-soft'}`}
    >
      {mobileLayout && (
        <div className="sticky top-0 z-[2] -mx-1.5 -mt-2 flex border-b border-stroke-soft bg-panel px-0.5 pb-2">
          <Button variant="ghost" className="min-h-11 justify-start px-2" onClick={onBackToList}>
            <ChevronLeft size={17} />
            {t('chat:sessionTree.backToNodes')}
          </Button>
        </div>
      )}
      {selected ? (
        <>
          {(parentSession || childSessions.length > 0) && (
            <div className="session-tree-lineage flex flex-col gap-1.5 border-b border-stroke-soft pb-3 [&_>_strong]:text-xs [&_>_strong]:font-medium [&_>_strong]:text-content-muted [&_button]:justify-start [&_button]:text-left [&_button]:text-xs">
              <strong>{t('chat:sessionTree.conversationLineage')}</strong>
              {parentSession && (
                <Button
                  variant="secondary"
                  disabled={Boolean(openingRelatedId)}
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
                  variant="secondary"
                  disabled={Boolean(openingRelatedId)}
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
          <div>
            <span className="session-tree-selection-kind mb-1 flex items-center justify-between gap-2 text-xs text-content-muted [&_em]:rounded-xs [&_em]:border [&_em]:border-star-border [&_em]:bg-star-soft [&_em]:px-1.5 [&_em]:py-0.5 [&_em]:font-medium [&_em]:text-star-strong [&_em]:not-italic">
              {typeLabel(selected)}
              {stateLabel(selected) && <em>{stateLabel(selected)}</em>}
            </span>
            <strong>{selected.label || typeLabel(selected)}</strong>
            <p>{selected.text || t('chat:sessionTree.noPreview')}</p>
          </div>
          <div className="chat-resource-inputs flex flex-col gap-3 [&_input[type='checkbox']]:size-4 [&_input[type='checkbox']]:[accent-color:var(--star)] [&_label]:grid [&_label]:gap-1.5 [&_label]:text-xs [&_label]:font-medium [&_label]:text-content-secondary">
            {canLabel && (
              <>
                <label>
                  <span>{t('chat:sessionTree.nodeLabel')}</span>
                  <Input
                    value={label}
                    maxLength={80}
                    placeholder={t('chat:sessionTree.labelPlaceholder')}
                    onChange={(event) => onLabelChange(event.target.value)}
                  />
                </label>
                <Button
                  variant="secondary"
                  disabled={savingLabel || streaming || dataStreaming}
                  onClick={onSaveLabel}
                >
                  <Tag size={14} />
                  {savingLabel
                    ? t('chat:sessionTree.savingLabel')
                    : t('chat:sessionTree.saveLabel')}
                </Button>
                {Boolean(selected.label) && (
                  <Button
                    variant="secondary"
                    disabled={savingLabel || streaming || dataStreaming}
                    onClick={onRemoveLabel}
                  >
                    <Trash2 size={14} />
                    {t('chat:sessionTree.removeLabel')}
                  </Button>
                )}
              </>
            )}
            {canLabel && (
              <Button
                variant="secondary"
                disabled={creatingChild || streaming || dataStreaming}
                onClick={onCreateChild}
              >
                <MessageSquarePlus size={14} />
                {creatingChild
                  ? t('chat:sessionTree.creatingChildChat')
                  : t('chat:sessionTree.createChildChat')}
              </Button>
            )}
            {canDerive && (
              <label className="session-tree-summary-option grid-cols-[minmax(0,1fr)_auto] items-center [&_small]:col-span-full [&_small]:leading-relaxed [&_small]:font-normal [&_small]:text-content-muted">
                <span>{t('chat:sessionTree.abandonedBranchSummary')}</span>
                <input
                  type="checkbox"
                  checked={summarize}
                  disabled={selected.leaf || streaming || dataStreaming}
                  onChange={(event) => onSummarizeChange(event.target.checked)}
                />
                <small>{t('chat:sessionTree.abandonedBranchSummaryHint')}</small>
              </label>
            )}
          </div>
          {canDerive && <p>{t('chat:sessionTree.sideEffectsRemain')}</p>}
          {selected.timestamp && (
            <p>
              {new Intl.DateTimeFormat(language, {
                dateStyle: 'medium',
                timeStyle: 'short',
              }).format(new Date(selected.timestamp))}
            </p>
          )}
          {error && (
            <p role="alert" className="text-danger!">
              {error}
            </p>
          )}
          {canDerive && (
            <Button
              className="chat-resource-confirm min-h-8 min-w-[112px] self-start bg-star text-[var(--on-accent)] hover:bg-star-hover max-[650px]:min-h-11"
              disabled={navigating || selected.leaf || streaming || dataStreaming}
              onClick={onNavigate}
            >
              <GitBranch size={15} />
              {navigating
                ? t('chat:sessionTree.navigating')
                : selected.leaf
                  ? t('chat:sessionTree.currentPosition')
                  : t('chat:sessionTree.continueFromNode')}
            </Button>
          )}
        </>
      ) : (
        <p>{loading ? t('chat:sessionTree.loading') : error || t('chat:sessionTree.chooseNode')}</p>
      )}
      {error && !selected && (
        <Button variant="secondary" onClick={onRefresh}>
          {t('chat:sessionTree.retry')}
        </Button>
      )}
    </div>
  )
}
