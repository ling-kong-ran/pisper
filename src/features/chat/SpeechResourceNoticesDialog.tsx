import { Download } from 'lucide-react'
import type { RefObject } from 'react'
import { speechResourceNotices as notices } from './speech-resource-notices'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'

export function SpeechResourceNoticesDialog({
  modelId,
  modelName,
  returnFocus,
  onClose,
}: {
  modelId: string
  modelName: string
  returnFocus: RefObject<HTMLButtonElement | null>
  onClose: () => void
}) {
  const { t } = useI18n()
  const model = notices.models.find((item) => item.id === modelId)
  if (!model) return null
  const licenseIds = new Set(model.groups.flatMap((group) => group.licenseIds))
  const licenses = notices.licenses.filter((license) => licenseIds.has(license.id))
  const download = () => {
    const blob = new Blob([JSON.stringify({ ...notices, models: [model], licenses }, null, 2)], {
      type: 'application/json;charset=utf-8',
    })
    const url = URL.createObjectURL(blob)
    const link = document.createElement('a')
    link.href = url
    link.download = `${model.id}-notices.json`
    link.click()
    window.setTimeout(() => URL.revokeObjectURL(url), 0)
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent
        className="z-[130] max-h-[85dvh] min-w-0 grid-cols-1 overflow-y-auto rounded-lg sm:max-w-2xl"
        overlayClassName="z-[129]"
        onEscapeKeyDown={(event) => event.stopPropagation()}
        onCloseAutoFocus={(event) => {
          event.preventDefault()
          if (returnFocus.current?.isConnected) returnFocus.current.focus()
        }}
      >
        <DialogHeader>
          <DialogTitle>{t('chat:speechModels.notices')}</DialogTitle>
          <DialogDescription className="break-words">
            {modelName} ({model.revision})
          </DialogDescription>
        </DialogHeader>
        <p className="break-words text-sm text-muted-foreground">{notices.scope}</p>
        <p className="break-words text-sm text-muted-foreground">{notices.licenseIdsMeaning}</p>
        <div className="min-w-0 divide-y">
          {model.groups.map((group) => (
            <details key={group.id} className="min-w-0 py-3">
              <summary className="cursor-pointer break-words text-sm font-medium">
                {group.id}
                {group.status === 'pending' && (
                  <span className="ml-2 text-xs font-normal text-amber-700 dark:text-amber-400">
                    {t('chat:speechModels.provenancePending')}
                  </span>
                )}
              </summary>
              <div className="mt-3 min-w-0 space-y-3 text-sm">
                <p className="break-words">{group.summary}</p>
                {group.pendingReasons.map((reason) => (
                  <p key={reason} className="break-words text-muted-foreground">
                    {reason}
                  </p>
                ))}
                <pre className="whitespace-pre-wrap break-all text-xs">
                  {JSON.stringify(
                    {
                      pathScope: group.pathScope,
                      paths: group.paths,
                      licenseIds: group.licenseIds,
                      evidence: group.evidence,
                    },
                    null,
                    2,
                  )}
                </pre>
              </div>
            </details>
          ))}
        </div>
        <details className="min-w-0 border-t pt-3">
          <summary className="cursor-pointer text-sm font-medium">
            {t('chat:speechModels.modifications')}
          </summary>
          <pre className="mt-3 whitespace-pre-wrap break-all text-xs">
            {JSON.stringify(model.modifications, null, 2)}
          </pre>
        </details>
        {licenses.map((license) => (
          <details key={license.id} className="min-w-0 border-t pt-3">
            <summary className="cursor-pointer break-words text-sm font-medium">
              {license.name} ({license.id})
            </summary>
            <pre className="mt-3 whitespace-pre-wrap break-words text-xs">{license.text}</pre>
          </details>
        ))}
        <div className="flex justify-end border-t pt-3">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                size="icon-sm"
                variant="outline"
                onClick={download}
                aria-label={t('chat:speechModels.downloadNotices')}
              >
                <Download />
              </Button>
            </TooltipTrigger>
            <TooltipContent>{t('chat:speechModels.downloadNotices')}</TooltipContent>
          </Tooltip>
        </div>
      </DialogContent>
    </Dialog>
  )
}
