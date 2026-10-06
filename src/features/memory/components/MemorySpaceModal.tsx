// 记忆空间编辑弹窗：创建或重命名空间。
import { useState } from 'react'
import type { FormEvent } from 'react'
import { Plus, RefreshCw, X } from 'lucide-react'
import { AppCardHeader, AppError } from '@/components/ui/app-primitives'
import { useI18n } from '@/app/i18n/use-i18n'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { apiJson } from '@/lib/http/api'
import type { MemorySpace } from '@/features/memory/model/memory-types'

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export function MemorySpaceModal({
  space,
  onClose,
  onSaved,
}: {
  space: MemorySpace | null
  onClose: () => void
  onSaved: (space: MemorySpace, message: string) => Promise<void>
}) {
  const { t } = useI18n()
  const [name, setName] = useState(space?.name || '')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (saving) return
    setSaving(true)
    setError('')
    try {
      const result = await apiJson<MemorySpace>(
        space ? `/api/memory/spaces/${encodeURIComponent(space.id)}` : '/api/memory/spaces',
        {
          method: space ? 'PATCH' : 'POST',
          body: JSON.stringify({ name, kind: 'custom' }),
        },
      )
      await onSaved(
        result,
        space
          ? t('memory:memoryPage.memorySpaceRenamed')
          : t('memory:memoryPage.memorySpaceCreated'),
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
        aria-label={
          space ? t('memory:memoryPage.renameMemorySpace') : t('memory:memoryPage.newMemorySpace')
        }
        className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)]"
        onSubmit={submit}
      >
        <AppCardHeader>
          <div>
            <h2>
              {space
                ? t('memory:memoryPage.renameMemorySpace')
                : t('memory:memoryPage.newMemorySpace')}
            </h2>
            <p>
              {t(
                'memory:memoryPage.giveEachThemeOrProjectItsOwnMemorySpaceSoDurableMemoriesHaveSomewhereToBelong',
              )}
            </p>
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
          {t('memory:memoryPage.memorySpaceName')}
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={t('memory:memoryPage.forExampleProductDesignGuidelines')}
            autoFocus
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
          <Button size="lg" disabled={saving || !name.trim()}>
            {saving ? <RefreshCw className="animate-spin" size={14} /> : <Plus size={14} />}
            {saving ? t('memory:memoryPage.saving') : t('memory:memoryPage.save')}
          </Button>
        </div>
      </form>
    </div>
  )
}
