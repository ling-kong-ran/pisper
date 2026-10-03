// 记忆节点编辑弹窗：创建或更新单条记忆。
import { useState } from 'react'
import type { FormEvent } from 'react'
import { Pencil, RefreshCw, X } from 'lucide-react'
import { AppCardHeader, AppError } from '@/components/ui/app-primitives'
import { AppSelect } from '@/components/common/AppSelect'
import { useI18n } from '@/app/i18n/use-i18n'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { apiJson } from '@/lib/http/api'
import { MEMORY_TYPES } from '@/features/memory/model/memory-galaxy-constants'
import { memoryTypeLabel } from '@/features/memory/model/memory-utils'
import { spaceLabel } from '@/features/memory/model/memory-utils'
import type { MemoryNode, MemorySpace, MemoryType } from '@/features/memory/model/memory-types'

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export function MemoryNodeModal({
  spaces,
  node,
  initialSpaceId,
  onClose,
  onSaved,
}: {
  spaces: MemorySpace[]
  node: MemoryNode | null
  initialSpaceId: string
  onClose: () => void
  onSaved: (message: string) => Promise<void>
}) {
  const { t } = useI18n()
  const [draft, setDraft] = useState({
    spaceId: node?.spaceId || initialSpaceId || spaces[0]?.id || '',
    title: node?.title || '',
    content: node?.content || '',
    type: node?.type || ('concept' as MemoryType),
    sourcePath: node?.sourcePath || '',
    importance: node?.importance ?? 0.5,
  })
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (saving) return
    setSaving(true)
    setError('')
    try {
      await apiJson(
        node ? `/api/memory/nodes/${encodeURIComponent(node.id)}` : '/api/memory/nodes',
        {
          method: node ? 'PATCH' : 'POST',
          body: JSON.stringify(draft),
        },
      )
      await onSaved(
        node ? t('memory:memoryPage.memoryUpdated') : t('memory:memoryPage.memoryCreated'),
      )
    } catch (saveError) {
      setError(errorMessage(saveError))
    } finally {
      setSaving(false)
    }
  }
  return (
    <div
      className="modal-backdrop max-[650px]:p-[8px] fixed z-[70] inset-0 grid place-items-center overflow-y-auto bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)] [padding:20px] [overscroll-behavior:contain] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => !saving && event.target === event.currentTarget && onClose()}
    >
      <form
        role="dialog"
        aria-modal="true"
        aria-label={node ? t('memory:memoryPage.editMemory') : t('memory:memoryPage.addMemory')}
        className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)]"
        onSubmit={submit}
      >
        <AppCardHeader>
          <div>
            <h2>{node ? t('memory:memoryPage.editMemory') : t('memory:memoryPage.addMemory')}</h2>
            <p>{t('memory:memoryPage.lightUpIdeasWorthKeepingAsMemoriesYouCanReturnToLater')}</p>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={t('memory:memoryPage.closeDialog')}
            disabled={saving}
            onClick={onClose}
          >
            <X size={17} />
          </Button>
        </AppCardHeader>
        <FieldLabel variant="control">
          {t('memory:memoryPage.memorySpace')}
          <AppSelect
            value={draft.spaceId}
            onChange={(event) => setDraft({ ...draft, spaceId: event.target.value })}
          >
            {spaces.map((space) => (
              <option value={space.id} key={space.id}>
                {spaceLabel(space, t)}
              </option>
            ))}
          </AppSelect>
        </FieldLabel>
        <FieldLabel variant="control">
          {t('memory:memoryPage.memoryTitle')}
          <input
            value={draft.title}
            onChange={(event) => setDraft({ ...draft, title: event.target.value })}
            placeholder={t('memory:memoryPage.forExampleProjectUIConstraints')}
          />
        </FieldLabel>
        <FieldLabel variant="control">
          {t('memory:memoryPage.memoryContent')}
          <textarea
            value={draft.content}
            onChange={(event) => setDraft({ ...draft, content: event.target.value })}
            placeholder={t('memory:memoryPage.recordStandaloneReusableMemoryForFutureChats')}
          />
        </FieldLabel>
        <div className="form-grid grid gap-[9px]">
          <FieldLabel variant="control">
            {t('memory:memoryPage.memoryType')}
            <AppSelect
              value={draft.type}
              onChange={(event) => setDraft({ ...draft, type: event.target.value as MemoryType })}
            >
              {MEMORY_TYPES.map((value) => (
                <option value={value} key={value}>
                  {memoryTypeLabel(value, t)}
                </option>
              ))}
            </AppSelect>
          </FieldLabel>
          <FieldLabel variant="control">
            {t('memory:memoryPage.importance')}
            <AppSelect
              value={draft.importance}
              onChange={(event) => setDraft({ ...draft, importance: Number(event.target.value) })}
            >
              <option value="0.3">{t('memory:memoryPage.normal')}</option>
              <option value="0.5">{t('memory:memoryPage.common')}</option>
              <option value="0.8">{t('memory:memoryPage.important')}</option>
              <option value="1">{t('memory:memoryPage.strict')}</option>
            </AppSelect>
          </FieldLabel>
        </div>
        <FieldLabel variant="control">
          {t('memory:memoryPage.relatedFilePath')}
          <input
            value={draft.sourcePath}
            onChange={(event) => setDraft({ ...draft, sourcePath: event.target.value })}
            placeholder={t('memory:memoryPage.optionalForExampleECodeProjectREADMEMd')}
          />
        </FieldLabel>
        {error && <AppError>{error}</AppError>}
        <div className="flex justify-end gap-[8px] [margin-top:18px]">
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            onClick={onClose}
          >
            {t('memory:memoryPage.cancel')}
          </Button>
          <Button
            size="lg"
            disabled={saving || !draft.spaceId || !draft.title.trim() || !draft.content.trim()}
          >
            {saving ? <RefreshCw className="animate-spin" size={14} /> : <Pencil size={14} />}
            {saving ? t('memory:memoryPage.saving') : t('memory:memoryPage.save')}
          </Button>
        </div>
      </form>
    </div>
  )
}
