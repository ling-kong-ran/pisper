import { useCallback, useEffect, useRef, useState } from 'react'
import { Channel } from '@tauri-apps/api/core'
import { ArrowLeft, LoaderCircle, QrCode, Radar, RefreshCw, ScanLine, Server } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
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

type DnsServiceRecord = {
  name: string
  fullName: string
  port?: number
  addresses: string[]
  txt: Record<string, true | null | number[]>
  isActive: boolean
}

type DnsBrowseMessage = {
  browseId: number
  service?: DnsServiceRecord
  reason?: string
}

type DiscoveredServer = {
  id: string
  name: string
  url: string
  fingerprint: string
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

function invokeScanner<T>(command: string, args?: unknown): Promise<T> {
  return invokeMobile<T>(`plugin:barcode-scanner|${command}`, args)
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

function isScannerCancellation(cause: unknown) {
  return /cancel(?:led|ed)/i.test(errorMessage(cause))
}

function nextPaint() {
  return new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  })
}

function txtString(value: true | null | number[] | undefined) {
  if (!Array.isArray(value)) return ''
  return new TextDecoder().decode(Uint8Array.from(value))
}

function discoveredServer(service: DnsServiceRecord): DiscoveredServer | null {
  if (!service.isActive || !service.port) return null
  const fingerprint = txtString(service.txt.fp)
  if (!/^SHA256:[0-9A-F]{64}$/i.test(fingerprint)) return null
  const address =
    service.addresses.find((candidate) => /^\d{1,3}(?:\.\d{1,3}){3}$/.test(candidate)) ||
    service.addresses.find((candidate) => candidate.includes(':'))
  if (!address) return null
  const host = address.includes(':') ? `[${address}]` : address
  const name = txtString(service.txt.name) || service.name || 'Pisper Desktop'
  const url = `https://${host}:${service.port}`
  return { id: `${service.fullName}:${url}`, name, url, fingerprint }
}

function wait(milliseconds: number) {
  return new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds))
}

export function MobilePairingDialog({ open, onOpenChange }: MobilePairingDialogProps) {
  const { t } = useI18n()
  const [busy, setBusy] = useState<string | null>(null)
  const [scannerActive, setScannerActive] = useState(false)
  const [discovered, setDiscovered] = useState<DiscoveredServer[]>([])
  const [discovering, setDiscovering] = useState(false)
  const [discoveryError, setDiscoveryError] = useState('')
  const [lanStatus, setLanStatus] = useState('')
  const [error, setError] = useState('')
  const scannerActiveRef = useRef(false)
  const scannerCancelledRef = useRef(false)
  const scannerNavigatingRef = useRef(false)
  const discoveryRunRef = useRef(0)
  const lanOperationRef = useRef('')

  const setScannerVisible = useCallback((active: boolean) => {
    scannerActiveRef.current = active
    setScannerActive(active)
  }, [])

  const cancelScan = useCallback(async () => {
    scannerCancelledRef.current = true
    if (scannerActiveRef.current) {
      await invokeScanner<void>('cancel').catch(() => undefined)
    }
    setScannerVisible(false)
    setBusy(null)
  }, [setScannerVisible])

  useEffect(() => {
    if (!scannerActive) return
    const root = document.getElementById('root')
    const changed: Array<{
      element: HTMLElement
      property: string
      value: string
      priority: string
    }> = []
    const setTemporaryStyle = (element: HTMLElement | null, property: string, value: string) => {
      if (!element) return
      changed.push({
        element,
        property,
        value: element.style.getPropertyValue(property),
        priority: element.style.getPropertyPriority(property),
      })
      element.style.setProperty(property, value, 'important')
    }

    // windowed 扫描把相机放在 WebView 后面，页面根节点必须暂时透明且不可见。
    setTemporaryStyle(document.documentElement, 'background', 'transparent')
    setTemporaryStyle(document.body, 'background', 'transparent')
    setTemporaryStyle(root, 'background', 'transparent')
    setTemporaryStyle(root, 'visibility', 'hidden')
    return () => {
      for (const { element, property, value, priority } of changed.reverse()) {
        if (value) element.style.setProperty(property, value, priority)
        else element.style.removeProperty(property)
      }
    }
  }, [scannerActive])

  useEffect(() => {
    if (!scannerActive) return
    const marker = `pisper-scanner-${Date.now()}`
    window.history.pushState({ ...window.history.state, pisperScanner: marker }, '')
    const handleBack = () => void cancelScan()
    window.addEventListener('popstate', handleBack)
    return () => {
      window.removeEventListener('popstate', handleBack)
      if (!scannerNavigatingRef.current && window.history.state?.pisperScanner === marker) {
        window.history.back()
      }
    }
  }, [cancelScan, scannerActive])

  const enterRemote = (state: PairedMobileState) => {
    if (!state.proxyUrl) throw new Error(t('config:mobileServer.pairFailed'))
    window.location.replace(state.proxyUrl)
  }

  const discoverLan = useCallback(async () => {
    const run = ++discoveryRunRef.current
    setDiscovering(true)
    setDiscoveryError('')
    setDiscovered([])
    const channel = new Channel<DnsBrowseMessage>()
    channel.onmessage = (message) => {
      if (discoveryRunRef.current !== run) return
      if (message.reason === 'permission-denied') {
        setDiscoveryError(t('config:mobileServer.localNetworkDenied'))
        return
      }
      if (message.reason?.startsWith('error:')) {
        setDiscoveryError(t('config:mobileServer.discoveryFailed'))
        return
      }
      if (!message.service) return
      if (!message.service.isActive) {
        setDiscovered((current) =>
          current.filter((server) => !server.id.startsWith(`${message.service?.fullName}:`)),
        )
        return
      }
      const server = discoveredServer(message.service)
      if (!server) return
      setDiscovered((current) => [
        ...current.filter((item) => item.fingerprint !== server.fingerprint),
        server,
      ])
    }

    let browseId: number | null = null
    try {
      await invokeMobile<void>('mobile_ensure_local_network_permission')
      const handle = await invokeMobile<{ browseId: number }>('plugin:dns-sd|browse_start', {
        options: {
          service: { type: 'pisper', protocol: 'tcp', domain: 'local' },
          timeoutMs: 4_000,
        },
        channel,
      })
      browseId = handle.browseId
      await wait(4_300)
    } catch (cause) {
      if (discoveryRunRef.current === run) {
        setDiscoveryError(
          errorMessage(cause).includes('local_network_permission_denied')
            ? t('config:mobileServer.localNetworkDenied')
            : t('config:mobileServer.discoveryFailed'),
        )
      }
    } finally {
      if (browseId !== null) {
        await invokeMobile('plugin:dns-sd|browse_stop', { browseId }).catch(() => undefined)
      }
      if (discoveryRunRef.current === run) setDiscovering(false)
    }
  }, [t])

  useEffect(() => {
    if (!open || scannerActive) return
    void discoverLan()
    return () => {
      discoveryRunRef.current += 1
    }
  }, [discoverLan, open, scannerActive])

  const cancelLanPairing = useCallback(async () => {
    const operationId = lanOperationRef.current
    lanOperationRef.current = ''
    if (operationId) {
      await invokeMobile('mobile_cancel_lan_pairing', { operationId }).catch(() => undefined)
    }
    setBusy(null)
    setLanStatus('')
  }, [])

  const pairLan = async (server: DiscoveredServer) => {
    const operationId =
      globalThis.crypto?.randomUUID?.() ||
      `lan-${Date.now()}-${Math.random().toString(16).slice(2)}`
    lanOperationRef.current = operationId
    setBusy(`lan:${server.id}`)
    setError('')
    setLanStatus(t('config:mobileServer.awaitingApproval', { name: server.name }))
    try {
      const state = await invokeMobile<PairedMobileState>('mobile_pair_lan', {
        operationId,
        name: server.name,
        url: server.url,
        fingerprint: server.fingerprint,
        deviceName: null,
      })
      if (lanOperationRef.current !== operationId) return
      enterRemote(state)
    } catch (cause) {
      if (lanOperationRef.current === operationId) setError(errorMessage(cause))
    } finally {
      if (lanOperationRef.current === operationId) {
        lanOperationRef.current = ''
        setBusy(null)
        setLanStatus('')
      }
    }
  }

  const scan = async () => {
    scannerCancelledRef.current = false
    scannerNavigatingRef.current = false
    setBusy('scan')
    setError('')
    try {
      let permission = await invokeScanner<BarcodePermissionState>('check_permissions')
      if (permission.camera !== 'granted') {
        permission = await invokeScanner<BarcodePermissionState>('request_permissions')
      }
      if (permission.camera !== 'granted') {
        throw new Error(t('config:mobileServer.cameraDenied'))
      }
      if (scannerCancelledRef.current) return

      setScannerVisible(true)
      await nextPaint()
      if (scannerCancelledRef.current) return
      const result = await invokeScanner<BarcodeResult>('scan', {
        windowed: true,
        // 插件会自动包含 QR_CODE；显式传入 QR_CODE 会被旧版 Android 实现映射为无效格式 0。
        formats: [],
      })
      if (!result.content) throw new Error(t('config:mobileServer.scanFailed'))
      scannerNavigatingRef.current = true
      enterRemote(
        await invokeMobile<PairedMobileState>('mobile_pair', {
          payloadJson: result.content,
          deviceName: null,
        }),
      )
    } catch (cause) {
      if (!scannerCancelledRef.current && !isScannerCancellation(cause)) {
        setError(errorMessage(cause))
      }
    } finally {
      setScannerVisible(false)
      setBusy(null)
      scannerCancelledRef.current = false
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen && scannerActive) {
          void cancelScan()
          return
        }
        if (!nextOpen && busy?.startsWith('lan:')) {
          void cancelLanPairing()
          onOpenChange(false)
          return
        }
        if (busy) return
        if (!nextOpen) setError('')
        onOpenChange(nextOpen)
      }}
    >
      <DialogContent
        showCloseButton={!scannerActive}
        overlayClassName={
          scannerActive ? 'bg-transparent supports-backdrop-filter:backdrop-blur-none' : undefined
        }
        className={
          scannerActive
            ? 'inset-0 top-0 left-0 h-dvh max-h-none w-screen max-w-none translate-x-0 translate-y-0 gap-0 overflow-hidden rounded-none bg-transparent p-0 text-white ring-0 sm:max-w-none'
            : 'sm:max-w-md'
        }
        onPointerDownOutside={scannerActive ? (event) => event.preventDefault() : undefined}
      >
        {scannerActive ? (
          <div className="flex h-dvh w-screen flex-col overflow-hidden">
            <DialogHeader className="sr-only">
              <DialogTitle>{t('config:mobileServer.scanTitle')}</DialogTitle>
              <DialogDescription>{t('config:mobileServer.scanHint')}</DialogDescription>
            </DialogHeader>
            <header
              className="flex min-h-16 items-center justify-between gap-3 bg-black/60 px-4 pb-3"
              style={{ paddingTop: 'max(env(safe-area-inset-top), 0.75rem)' }}
            >
              <Button
                variant="ghost"
                className="h-10 bg-black/40 px-3 text-white hover:bg-black/70 hover:text-white"
                onClick={() => void cancelScan()}
              >
                <ArrowLeft />
                {t('config:mobileServer.cancelScan')}
              </Button>
              <div className="min-w-0 truncate text-sm font-medium">
                {t('config:mobileServer.scanTitle')}
              </div>
              <div className="w-[88px]" aria-hidden="true" />
            </header>
            <main className="grid min-h-0 flex-1 place-items-center p-6">
              <div className="relative aspect-square w-[min(72vw,20rem)] rounded-md border-2 border-white/90 shadow-[0_0_0_9999px_rgba(0,0,0,0.18)]">
                <ScanLine className="absolute top-1/2 left-1/2 size-10 -translate-x-1/2 -translate-y-1/2 text-white/75" />
              </div>
            </main>
            <footer
              className="flex flex-col items-center gap-3 bg-black/60 px-5 pt-4 text-center"
              style={{ paddingBottom: 'max(env(safe-area-inset-bottom), 1rem)' }}
            >
              <p className="max-w-sm text-sm leading-relaxed text-white/90">
                {t('config:mobileServer.scanHint')}
              </p>
              <Button
                variant="outline"
                className="w-full max-w-sm border-white/50 bg-black/30 text-white hover:bg-black/60 hover:text-white"
                onClick={() => void cancelScan()}
              >
                {t('config:mobileServer.cancelScan')}
              </Button>
            </footer>
          </div>
        ) : (
          <>
            <DialogHeader>
              <DialogTitle>{t('config:mobileServer.addServer')}</DialogTitle>
              <DialogDescription>{t('config:mobileServer.discoverDescription')}</DialogDescription>
            </DialogHeader>

            <section className="grid gap-2" aria-live="polite">
              <div className="flex items-center justify-between gap-3">
                <div className="flex items-center gap-2 text-sm font-medium">
                  <Radar className="size-4" />
                  {t('config:mobileServer.nearby')}
                </div>
                <Button
                  size="icon-sm"
                  variant="ghost"
                  disabled={discovering || busy !== null}
                  aria-label={t('config:mobileServer.rediscover')}
                  onClick={() => void discoverLan()}
                >
                  <RefreshCw className={discovering ? 'animate-spin' : undefined} />
                </Button>
              </div>
              {discovered.length ? (
                <ul className="grid max-h-44 gap-1.5 overflow-y-auto">
                  {discovered.map((server) => {
                    const connecting = busy === `lan:${server.id}`
                    return (
                      <li
                        key={server.id}
                        className="flex min-h-12 items-center justify-between gap-3 rounded-md border px-3 py-2"
                      >
                        <div className="flex min-w-0 items-center gap-2">
                          <Server className="size-4 flex-none text-muted-foreground" />
                          <div className="min-w-0">
                            <div className="truncate text-sm">{server.name}</div>
                            <div className="truncate text-xs text-muted-foreground">
                              {server.url}
                            </div>
                          </div>
                        </div>
                        <Button
                          size="sm"
                          disabled={busy !== null}
                          onClick={() => void pairLan(server)}
                        >
                          {connecting ? <LoaderCircle className="animate-spin" /> : null}
                          {t('config:mobileServer.requestConnect')}
                        </Button>
                      </li>
                    )
                  })}
                </ul>
              ) : (
                <p className="text-xs leading-relaxed text-muted-foreground">
                  {discovering
                    ? t('config:mobileServer.discovering')
                    : t('config:mobileServer.noNearby')}
                </p>
              )}
              {discoveryError ? (
                <p className="text-xs leading-relaxed text-destructive">{discoveryError}</p>
              ) : null}
              {lanStatus ? (
                <div className="flex items-center justify-between gap-3 rounded-md bg-muted px-3 py-2">
                  <p className="text-xs leading-relaxed">{lanStatus}</p>
                  <Button size="sm" variant="outline" onClick={() => void cancelLanPairing()}>
                    {t('config:mobileServer.cancelRequest')}
                  </Button>
                </div>
              ) : null}
            </section>

            <div className="relative py-1">
              <Separator />
              <span className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 bg-popover px-2 text-xs text-muted-foreground">
                {t('config:mobileServer.otherMethods')}
              </span>
            </div>

            <Button className="w-full" disabled={busy !== null} onClick={() => void scan()}>
              {busy === 'scan' ? <LoaderCircle className="animate-spin" /> : <QrCode />}
              {t('config:mobileServer.scan')}
            </Button>

            {error ? <p className="text-xs leading-relaxed text-destructive">{error}</p> : null}
          </>
        )}
      </DialogContent>
    </Dialog>
  )
}
