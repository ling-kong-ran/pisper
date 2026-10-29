import { useEffect, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Copy, KeyRound, RefreshCw, Server } from 'lucide-react'

import { useI18n } from '@/app/use-i18n'
import { Alert } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

import { mcpHostApi, type McpHostCredentials } from './mcp-host-api'

const MCP_HOST_QUERY_KEY = ['mcp-host', 'status'] as const

function connectionConfig(credentials: McpHostCredentials) {
  return JSON.stringify(
    {
      mcpServers: {
        pisper: {
          url: credentials.url,
          headers: { Authorization: `Bearer ${credentials.token}` },
        },
      },
    },
    null,
    2,
  )
}

export function McpHostPanel({
  notify,
  requestConfirm,
}: {
  notify: Notify
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t } = useI18n()
  const queryClient = useQueryClient()
  const [updating, setUpdating] = useState(false)
  const [revealing, setRevealing] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [credentials, setCredentials] = useState<McpHostCredentials | null>(null)
  const [credentialError, setCredentialError] = useState('')
  const credentialRequest = useRef<AbortController | null>(null)
  const configRef = useRef<HTMLTextAreaElement>(null)
  const unavailableOnMobile = typeof window !== 'undefined' && Boolean(window.__PISPER_MOBILE_APP__)
  const statusQuery = useQuery({
    queryKey: MCP_HOST_QUERY_KEY,
    queryFn: ({ signal }) => mcpHostApi.status(signal),
    enabled: !unavailableOnMobile && !updating,
    refetchInterval: 15_000,
  })

  useEffect(
    () => () => {
      credentialRequest.current?.abort()
    },
    [],
  )

  if (unavailableOnMobile) return null

  const status = statusQuery.data
  const config = credentials ? connectionConfig(credentials) : ''
  const closeDialog = () => {
    credentialRequest.current?.abort()
    credentialRequest.current = null
    setCredentials(null)
    setCredentialError('')
    setDialogOpen(false)
  }
  const loadCredentials = async () => {
    credentialRequest.current?.abort()
    const controller = new AbortController()
    credentialRequest.current = controller
    setRevealing(true)
    setCredentials(null)
    setCredentialError('')
    setDialogOpen(true)
    try {
      const result = await mcpHostApi.credentials(controller.signal)
      if (!controller.signal.aborted) setCredentials(result)
    } catch (error) {
      if (!controller.signal.aborted)
        setCredentialError(error instanceof Error ? error.message : t('mcp:hostLoadFailed'))
    } finally {
      if (credentialRequest.current === controller) {
        credentialRequest.current = null
        setRevealing(false)
      }
    }
  }
  const toggle = async (enabled: boolean) => {
    setUpdating(true)
    try {
      const result = await mcpHostApi.setEnabled(enabled)
      queryClient.setQueryData(MCP_HOST_QUERY_KEY, result)
      if (!enabled) closeDialog()
      notify(
        result.error || (enabled ? t('mcp:hostEnabledNotice') : t('mcp:hostDisabledNotice')),
        result.error ? 'error' : 'success',
      )
    } catch (error) {
      notify(error instanceof Error ? error.message : t('mcp:hostUpdateFailed'), 'error')
    } finally {
      setUpdating(false)
    }
  }
  const rotate = async () => {
    if (
      requestConfirm &&
      !(await requestConfirm({
        title: t('mcp:hostRotateTitle'),
        message: t('mcp:hostRotateWarning'),
        confirmLabel: t('mcp:hostRotate'),
        tone: 'danger',
      }))
    )
      return
    credentialRequest.current?.abort()
    const controller = new AbortController()
    credentialRequest.current = controller
    setRevealing(true)
    setCredentialError('')
    setCredentials(null)
    try {
      const result = await mcpHostApi.rotateToken(controller.signal)
      if (!controller.signal.aborted) {
        setCredentials(result)
        notify(t('mcp:hostRotatedNotice'), 'success')
      }
    } catch (error) {
      if (!controller.signal.aborted)
        setCredentialError(error instanceof Error ? error.message : t('mcp:hostRotateFailed'))
    } finally {
      if (credentialRequest.current === controller) {
        credentialRequest.current = null
        setRevealing(false)
      }
    }
  }
  const copyConfig = async () => {
    try {
      await navigator.clipboard.writeText(config)
      notify(t('mcp:hostCopied'), 'success')
    } catch {
      configRef.current?.select()
      notify(t('mcp:hostCopyManually'), 'error')
    }
  }

  return (
    <>
      {/* 内置服务是整页的状态带：位于列表/详情之上，细线分隔，不再叠一层卡片。 */}
      <section className="flex shrink-0 flex-col gap-2.5 border-b border-stroke-soft px-4 py-3 @min-[760px]/page:px-6">
        <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
          <div className="flex min-w-0 items-center gap-3">
            <span
              className="grid size-8 shrink-0 place-items-center rounded-sm border border-key-border bg-key text-content-secondary shadow-key"
              aria-hidden="true"
            >
              <Server className="size-4" />
            </span>
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-2">
                <h2 className="text-sm font-medium text-content">{t('mcp:hostTitle')}</h2>
                {status && (
                  <Badge variant={status.listening ? 'ok' : status.enabled ? 'bad' : 'default'}>
                    {status.listening
                      ? t('mcp:hostReady')
                      : status.enabled
                        ? t('mcp:hostUnavailable')
                        : t('mcp:disabled')}
                  </Badge>
                )}
              </div>
              <p className="max-w-2xl text-xs leading-5 text-content-muted">
                {t('mcp:hostDescription')}
              </p>
            </div>
          </div>
          <label className="flex shrink-0 items-center gap-2.5 text-xs text-content-secondary">
            {t('mcp:hostEnabled')}
            <Switch
              checked={Boolean(status?.enabled)}
              disabled={!status || updating}
              aria-label={t('mcp:hostEnabled')}
              onCheckedChange={(enabled) => void toggle(enabled)}
            />
          </label>
        </div>
        {statusQuery.isPending && <p className="text-xs text-content-muted">{t('mcp:loading')}</p>}
        {statusQuery.error && (
          <Alert className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
            <span>{statusQuery.error.message}</span>
            <Button size="sm" variant="ghost" onClick={() => void statusQuery.refetch()}>
              {t('mcp:retry')}
            </Button>
          </Alert>
        )}
        {status?.error && (
          <Alert role="status" className="text-xs text-muted-foreground">
            {status.error}
          </Alert>
        )}
        {status?.listening && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2 pl-11 max-[650px]:pl-0">
            <code className="max-w-full min-w-0 rounded-xs border border-stroke bg-code px-2 py-0.5 font-mono text-xs break-all text-content">
              {status.url}
            </code>
            <Button
              size="sm"
              variant="secondary"
              className="max-[650px]:h-11"
              onClick={() => void loadCredentials()}
            >
              <KeyRound data-icon="inline-start" />
              {t('mcp:hostShowConfig')}
            </Button>
          </div>
        )}
      </section>
      <Dialog open={dialogOpen} onOpenChange={(open) => !open && closeDialog()}>
        <DialogContent className="min-w-0 grid-cols-[minmax(0,1fr)] sm:max-w-xl">
          <DialogHeader className="min-w-0 pr-8 text-left">
            <DialogTitle>{t('mcp:hostConfigTitle')}</DialogTitle>
            <DialogDescription className="leading-5">
              {t('mcp:hostConfigDescription')}
            </DialogDescription>
          </DialogHeader>
          {revealing && (
            <p role="status" className="flex items-center gap-2 text-sm text-content-muted">
              <OrbitSpinner />
              {t('mcp:hostLoadingConfig')}
            </p>
          )}
          {credentialError && (
            <Alert role="alert" className="text-sm text-muted-foreground">
              {credentialError}
            </Alert>
          )}
          {credentials && (
            <Textarea
              ref={configRef}
              value={config}
              readOnly
              spellCheck={false}
              aria-label={t('mcp:hostConfigTitle')}
              className="h-52 max-h-[45dvh] resize-y bg-code font-mono text-xs [overflow-wrap:anywhere]"
              onFocus={(event) => event.currentTarget.select()}
            />
          )}
          <DialogFooter className="min-w-0 flex-wrap">
            <Button variant="secondary" disabled={revealing} onClick={() => void rotate()}>
              <RefreshCw data-icon="inline-start" />
              {t('mcp:hostRotate')}
            </Button>
            <Button disabled={!credentials || revealing} onClick={() => void copyConfig()}>
              <Copy data-icon="inline-start" />
              {t('mcp:hostCopyConfig')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
