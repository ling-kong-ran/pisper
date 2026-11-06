// 选远程机器时填对方机器上的目录：确认前就连过去检查，连不上、目录不存在都在这里说清楚，不等到发第一条消息。
import './machines-messages'
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { AlertTriangle, Server } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { FieldDescription, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import { machineFailureMessage } from './machine-messages'
import { machineErrorCode, resolveMachineFolder, type Machine } from './machines-api'

export function MachineFolderDialog({
  machine,
  initialCwd,
  onOpenChange,
  onChosen,
}: {
  // 空表示对话框关闭。
  machine: Machine | null
  initialCwd: string
  onOpenChange: (open: boolean) => void
  onChosen: (machine: Machine, cwd: string) => void
}) {
  const { t } = useI18n()
  const [cwd, setCwd] = useState(initialCwd)
  const [checking, setChecking] = useState(false)
  const [error, setError] = useState('')
  const inFlight = useRef<AbortController | null>(null)

  useEffect(() => {
    if (machine) {
      setCwd(initialCwd)
      setError('')
    }
    return () => {
      inFlight.current?.abort()
      inFlight.current = null
      setChecking(false)
    }
  }, [machine, initialCwd])

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (!machine || checking) return
    const controller = new AbortController()
    inFlight.current = controller
    setChecking(true)
    setError('')
    try {
      const resolved = await resolveMachineFolder(machine.id, cwd.trim(), controller.signal)
      if (!controller.signal.aborted) onChosen(machine, resolved)
    } catch (caught) {
      if (controller.signal.aborted) return
      setError(
        machineFailureMessage(
          t,
          machineErrorCode(caught),
          caught instanceof Error ? caught.message : '',
        ),
      )
    } finally {
      if (!controller.signal.aborted) setChecking(false)
    }
  }

  return (
    <Dialog open={Boolean(machine)} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <form onSubmit={(event) => void submit(event)} className="grid gap-4">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <Server className="size-4" aria-hidden="true" />
              {t('board:machine.folderTitle', { name: machine?.name ?? '' })}
            </DialogTitle>
            <DialogDescription>{t('board:machine.folderDescription')}</DialogDescription>
          </DialogHeader>
          <div className="grid gap-1.5">
            <FieldLabel htmlFor="machine-folder">{t('board:machine.folderLabel')}</FieldLabel>
            <Input
              id="machine-folder"
              value={cwd}
              autoFocus
              spellCheck={false}
              className="font-mono text-xs"
              placeholder={machine?.roots[0] || '~'}
              disabled={checking}
              data-machine-folder
              onChange={(event) => setCwd(event.target.value)}
            />
            <FieldDescription>
              {machine?.kind === 'node'
                ? t('board:machine.folderInRoots', { roots: machine.roots.join('、') })
                : t('board:machine.folderHint')}
            </FieldDescription>
          </div>
          {error && (
            <Alert variant="destructive" data-machine-folder-error>
              <AlertTriangle aria-hidden="true" />
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              {t('board:machine.cancel')}
            </Button>
            <Button type="submit" disabled={checking} data-machine-folder-confirm>
              {checking && <OrbitSpinner className="size-3.5" />}
              {checking ? t('board:machine.checking') : t('board:machine.confirm')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
