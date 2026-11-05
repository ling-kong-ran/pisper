// 添加 / 编辑记忆与新建 / 重命名空间的对话框。保存中不能关闭（Dialog 的 busy），失败写在表单里。
import { useState, type FormEvent } from 'react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { AppError } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import { Textarea } from '@/components/ui/textarea'
import {
  createMemory,
  createMemorySpace,
  renameMemorySpace,
  updateMemory,
  type MemoryDraft,
} from './memory-api'
import { memorySpaceLabel, memoryTypeLabel } from './memory-labels'
import { MEMORY_TYPES, type MemoryNode, type MemorySpace, type MemoryType } from './memory-types'

const IMPORTANCE_LEVELS = [0.3, 0.5, 0.8, 1]

// 下拉只有这几档；老数据或模型写的值落到最接近的一档，不显示成空。
function nearestImportance(value: number | undefined) {
  const target = value ?? 0.5
  return IMPORTANCE_LEVELS.reduce((best, level) =>
    Math.abs(level - target) < Math.abs(best - target) ? level : best,
  )
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export function MemoryEditDialog({
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
  onSaved: (message: string) => Promise<void> | void
}) {
  const { t } = useI18n()
  const [draft, setDraft] = useState<MemoryDraft>({
    spaceId: node?.spaceId || initialSpaceId || spaces[0]?.id || '',
    title: node?.title || '',
    content: node?.content || '',
    type: node?.type || 'concept',
    sourcePath: node?.sourcePath || '',
    importance: nearestImportance(node?.importance),
  })
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const heading = node ? t('memory:memoryPage.editMemory') : t('memory:memoryPage.addMemory')
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (saving) return
    setSaving(true)
    setError('')
    try {
      if (node) await updateMemory(node.id, draft)
      else await createMemory(draft)
      await onSaved(
        node ? t('memory:memoryPage.memoryUpdated') : t('memory:memoryPage.memoryCreated'),
      )
    } catch (saveError) {
      setError(errorText(saveError))
    } finally {
      setSaving(false)
    }
  }
  return (
    <Dialog open busy={saving} onOpenChange={(open) => !open && onClose()}>
      <DialogContent size="compact">
        <form className="grid min-w-0 gap-3" onSubmit={(event) => void submit(event)}>
          <DialogHeader>
            <DialogTitle>{heading}</DialogTitle>
            <DialogDescription>{t('memory:memoryPage.memoryDialogHint')}</DialogDescription>
          </DialogHeader>
          <FieldLabel variant="control">
            {t('memory:memoryPage.memoryTitle')}
            <Input
              value={draft.title}
              onChange={(event) => setDraft({ ...draft, title: event.target.value })}
              placeholder={t('memory:memoryPage.forExampleProjectUIConstraints')}
            />
          </FieldLabel>
          <FieldLabel variant="control">
            {t('memory:memoryPage.memoryContent')}
            <Textarea
              value={draft.content}
              onChange={(event) => setDraft({ ...draft, content: event.target.value })}
              placeholder={t('memory:memoryPage.recordStandaloneReusableMemoryForFutureChats')}
            />
          </FieldLabel>
          <FieldLabel variant="control">
            {t('memory:memoryPage.memorySpace')}
            <AppSelect
              value={draft.spaceId}
              onChange={(event) => setDraft({ ...draft, spaceId: event.target.value })}
            >
              {spaces.map((space) => (
                <option value={space.id} key={space.id}>
                  {memorySpaceLabel(space, t)}
                </option>
              ))}
            </AppSelect>
          </FieldLabel>
          <div className="grid min-w-0 gap-x-2.5 gap-y-3 sm:grid-cols-2">
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
            <Input
              value={draft.sourcePath}
              onChange={(event) => setDraft({ ...draft, sourcePath: event.target.value })}
              placeholder={t('memory:memoryPage.optionalForExampleECodeProjectREADMEMd')}
            />
          </FieldLabel>
          {error && <AppError>{error}</AppError>}
          <DialogFooter>
            <Button
              type="button"
              variant="secondary"
              size="dialogAction"
              disabled={saving}
              onClick={onClose}
            >
              {t('memory:memoryPage.cancel')}
            </Button>
            <Button
              type="submit"
              size="dialogAction"
              disabled={saving || !draft.spaceId || !draft.title.trim() || !draft.content.trim()}
            >
              {saving && <OrbitSpinner className="text-current" />}
              {saving ? t('memory:memoryPage.saving') : t('memory:memoryPage.save')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

export function MemorySpaceDialog({
  space,
  onClose,
  onSaved,
}: {
  space: MemorySpace | null
  onClose: () => void
  onSaved: (space: MemorySpace, message: string) => Promise<void> | void
}) {
  const { t } = useI18n()
  const [name, setName] = useState(space?.name || '')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const heading = space
    ? t('memory:memoryPage.renameMemorySpace')
    : t('memory:memoryPage.newMemorySpace')
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (saving) return
    setSaving(true)
    setError('')
    try {
      const result = space ? await renameMemorySpace(space.id, name) : await createMemorySpace(name)
      await onSaved(
        result,
        space
          ? t('memory:memoryPage.memorySpaceRenamed')
          : t('memory:memoryPage.memorySpaceCreated'),
      )
    } catch (saveError) {
      setError(errorText(saveError))
    } finally {
      setSaving(false)
    }
  }
  return (
    <Dialog open busy={saving} onOpenChange={(open) => !open && onClose()}>
      <DialogContent size="compact">
        <form className="grid min-w-0 gap-3" onSubmit={(event) => void submit(event)}>
          <DialogHeader>
            <DialogTitle>{heading}</DialogTitle>
            <DialogDescription>{t('memory:memoryPage.memorySpaceHint')}</DialogDescription>
          </DialogHeader>
          <FieldLabel variant="control">
            {t('memory:memoryPage.memorySpaceName')}
            <Input
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder={t('memory:memoryPage.forExampleProductDesignGuidelines')}
              autoFocus
            />
          </FieldLabel>
          {error && <AppError>{error}</AppError>}
          <DialogFooter>
            <Button
              type="button"
              variant="secondary"
              size="dialogAction"
              disabled={saving}
              onClick={onClose}
            >
              {t('memory:memoryPage.cancel')}
            </Button>
            <Button type="submit" size="dialogAction" disabled={saving || !name.trim()}>
              {saving && <OrbitSpinner className="text-current" />}
              {saving ? t('memory:memoryPage.saving') : t('memory:memoryPage.save')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
