import { useState } from 'react'
import { LoaderCircle, QrCode } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Separator } from '@/components/ui/separator'

type PairedMobileState = {
  proxyUrl: string
}

type BarcodePermissionState = {
  camera?: string
}

type BarcodeResult = {
  content?: string
}

type MobilePairingDialogProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
}

function invokeMobile<T>(command: string, args?: unknown): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('native bridge unavailable'))
  return invoke<T>(command, args)
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

export function MobilePairingDialog({ open, onOpenChange }: MobilePairingDialogProps) {
  const { t } = useI18n()
  const [url, setUrl] = useState('')
  const [code, setCode] = useState('')
  const [fingerprint, setFingerprint] = useState('')
  const [busy, setBusy] = useState<'scan' | 'manual' | null>(null)
  const [error, setError] = useState('')

  const enterRemote = (state: PairedMobileState) => {
    if (!state.proxyUrl) throw new Error(t('config:mobileServer.pairFailed'))
    window.location.replace(state.proxyUrl)
  }

  const scan = async () => {
    setBusy('scan')
    setError('')
    try {
      const scanner = <T,>(command: string, args?: unknown) =>
        invokeMobile<T>(`plugin:barcode-scanner|${command}`, args)
      let permission = await scanner<BarcodePermissionState>('check_permissions')
      if (permission.camera !== 'granted') {
        permission = await scanner<BarcodePermissionState>('request_permissions')
      }
      if (permission.camera !== 'granted') {
        throw new Error(t('config:mobileServer.cameraDenied'))
      }
      const result = await scanner<BarcodeResult>('scan', {
        windowed: false,
        formats: ['QR_CODE'],
      })
      if (!result.content) throw new Error(t('config:mobileServer.scanFailed'))
      enterRemote(
        await invokeMobile<PairedMobileState>('mobile_pair', {
          payloadJson: result.content,
          deviceName: null,
        }),
      )
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(null)
    }
  }

  const pairManually = async () => {
    setBusy('manual')
    setError('')
    try {
      enterRemote(
        await invokeMobile<PairedMobileState>('mobile_pair_manual', {
          url,
          code,
          fingerprint,
          deviceName: null,
        }),
      )
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(null)
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        if (busy) return
        if (!nextOpen) setError('')
        onOpenChange(nextOpen)
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t('config:mobileServer.addServer')}</DialogTitle>
          <DialogDescription className="sr-only">
            {t('config:mobileServer.description')}
          </DialogDescription>
        </DialogHeader>

        <Button className="w-full" disabled={busy !== null} onClick={() => void scan()}>
          {busy === 'scan' ? <LoaderCircle className="animate-spin" /> : <QrCode />}
          {t('config:mobileServer.scan')}
        </Button>

        <div className="relative py-1">
          <Separator />
          <span className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 bg-popover px-2 text-xs text-muted-foreground">
            {t('config:mobileServer.manual')}
          </span>
        </div>

        <form
          className="grid gap-3"
          onSubmit={(event) => {
            event.preventDefault()
            void pairManually()
          }}
        >
          <div className="grid gap-1.5">
            <Label htmlFor="mobile-pair-url">{t('config:mobileServer.url')}</Label>
            <Input
              id="mobile-pair-url"
              inputMode="url"
              autoCapitalize="off"
              autoCorrect="off"
              placeholder="https://192.168.1.5:5174"
              value={url}
              onChange={(event) => setUrl(event.target.value)}
            />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="mobile-pair-code">{t('config:mobileServer.code')}</Label>
            <Input
              id="mobile-pair-code"
              autoCapitalize="characters"
              autoCorrect="off"
              placeholder="ABCD-EFGH"
              value={code}
              onChange={(event) => setCode(event.target.value)}
            />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="mobile-pair-fingerprint">{t('config:remoteAccess.fingerprint')}</Label>
            <Input
              id="mobile-pair-fingerprint"
              autoCapitalize="characters"
              autoCorrect="off"
              placeholder="SHA256:1BA3FEB1"
              value={fingerprint}
              onChange={(event) => setFingerprint(event.target.value)}
            />
          </div>
          {error ? <p className="text-xs leading-relaxed text-destructive">{error}</p> : null}
          <DialogFooter className="mt-1">
            <Button type="submit" disabled={busy !== null || !url || !code || !fingerprint}>
              {busy === 'manual' ? <LoaderCircle className="animate-spin" /> : null}
              {t('config:mobileServer.pair')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
