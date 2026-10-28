import { useEffect, useRef, useState, type FormEvent } from 'react'
import { Plus } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { FieldLabel } from '@/components/ui/field'
import { assetsApi } from './assets-api'

export function AssetLinkDialog({
  onClose,
  onCreated,
}: {
  onClose: () => void
  onCreated: () => void
}) {
  const { t } = useI18n()
  const [name, setName] = useState('')
  const [url, setUrl] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const savingRef = useRef(false)
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (savingRef.current) return
    savingRef.current = true
    setSaving(true)
    setError('')
    let accepted = false
    try {
      await assetsApi.createLink({ name, url })
      accepted = true
    } catch (caught) {
      if (mounted.current) setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      savingRef.current = false
      if (mounted.current) setSaving(false)
    }
    // POST 已接受后只失效读取，不把刷新失败变成重发创建。
    if (accepted) onCreated()
  }
  return (
    <Dialog
      open
      busy={saving}
      onOpenChange={(open) => {
        if (!open && !savingRef.current) onClose()
      }}
    >
      <DialogContent size="compact" scrollMode="body">
        <form
          className="flex min-h-0 flex-col gap-4"
          onSubmit={submit}
          aria-busy={saving || undefined}
        >
          <DialogHeader className="pr-8">
            <DialogTitle>{t('assets:assetsPage.addLinkAsset')}</DialogTitle>
            <DialogDescription>
              {t('assets:assetsPage.linksCanBeArchivedOpenedOrAddedToAChatAsContext')}
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="flex flex-col gap-4">
            <FieldLabel variant="control">
              {t('assets:assetsPage.name')}
              <input
                value={name}
                disabled={saving}
                onChange={(event) => setName(event.target.value)}
                placeholder={t('assets:assetsPage.forExampleOpenAIAPIDocs')}
              />
            </FieldLabel>
            <FieldLabel variant="control">
              URL
              <input
                value={url}
                disabled={saving}
                onChange={(event) => setUrl(event.target.value)}
                placeholder="https://example.com/docs"
              />
            </FieldLabel>
            {error && (
              <p role="alert" className="break-words text-sm text-danger">
                {error}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button variant="outline" size="dialogAction" disabled={saving} onClick={onClose}>
              {t('assets:assetsPage.cancel')}
            </Button>
            <Button
              type="submit"
              size="dialogAction"
              loading={saving}
              disabled={saving || !url.trim()}
            >
              <Plus />
              {saving ? t('assets:assetsPage.adding') : t('assets:assetsPage.addLink')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
