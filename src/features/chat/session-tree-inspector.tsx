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
      className={`chat-resource-config flex min-h-0 min-w-0 flex-col gap-[16px] overflow-y-auto [overscroll-behavior:contain] [padding:18px] session-tree-inspector [&_>_p]:m-[auto] [&_>_p]:text-[12px] [&_>_p]:text-[var(--text-muted)] [&_strong]:text-[14px] [&_p]:text-[12px] [&_p]:text-[var(--text-muted)] ${mobileLayout ? 'bg-[var(--solid)]' : '[margin:12px_12px_12px_0] rounded-[var(--r-md)] bg-[color-mix(in_srgb,var(--solid)_88%,var(--surface-subtle))] shadow-[0_10px_28px_-26px_var(--shadow)]'}`}
    >
      {mobileLayout && (
        <div className="sticky top-0 z-[2] -mx-[6px] -mt-[8px] flex border-b border-[var(--stroke-soft)] bg-[var(--solid)] px-[2px] pb-[8px]">
          <Button
            variant="ghost"
            className="min-h-[40px] justify-start px-[8px]"
            onClick={onBackToList}
          >
            <ChevronLeft size={17} />
            {t('chat:sessionTree.backToNodes')}
          </Button>
        </div>
      )}
      {selected ? (
        <>
          {(parentSession || childSessions.length > 0) && (
            <div className="session-tree-lineage [&_>_strong]:text-[12px] [&_>_strong]:font-[650] [&_button]:justify-start [&_button]:text-left [&_button]:text-[11px] flex flex-col gap-[6px] [border-bottom:1px_solid_var(--stroke-soft)] [padding-bottom:12px]">
              <strong>{t('chat:sessionTree.conversationLineage')}</strong>
              {parentSession && (
                <Button
                  variant="outline"
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
                  variant="outline"
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
            <span className="session-tree-selection-kind [&_em]:rounded-[var(--r-xs)] [&_em]:bg-[var(--brand-blue-soft)] [&_em]:text-[var(--brand-blue-strong)] [&_em]:p-[2px_5px] [&_em]:[font-style:normal] flex items-center justify-between gap-[8px] text-[var(--text-muted)] text-[10px] [text-transform:uppercase]">
              {typeLabel(selected)}
              {stateLabel(selected) && <em>{stateLabel(selected)}</em>}
            </span>
            <strong>{selected.label || typeLabel(selected)}</strong>
            <p>{selected.text || t('chat:sessionTree.noPreview')}</p>
          </div>
          <div className="chat-resource-inputs [&_label]:grid [&_label]:gap-[5px] [&_label]:text-[12px] [&_label]:font-[600] [&_input[type='checkbox']]:w-[16px] [&_input[type='checkbox']]:h-[16px] [&_input[type='checkbox']]:[accent-color:var(--blue)] flex flex-col gap-[11px]">
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
                  variant="outline"
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
                    variant="outline"
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
                variant="outline"
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
              <label className="session-tree-summary-option [&_small]:[grid-column:1/-1] [&_small]:text-[var(--text-muted)] [&_small]:font-[400] [&_small]:leading-[1.45] grid-cols-[minmax(0,1fr)_auto] items-center">
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
          {error && <p className="danger-text">{error}</p>}
          {canDerive && (
            <Button
              className="chat-resource-confirm hover:bg-[var(--star-hover)] hover:text-[var(--on-accent)] min-w-[112px] min-h-[36px] self-start bg-[var(--star)] text-[var(--on-accent)] font-[650]"
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
        <Button variant="outline" onClick={onRefresh}>
          {t('chat:sessionTree.retry')}
        </Button>
      )}
    </div>
  )
}
