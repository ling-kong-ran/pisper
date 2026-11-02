import { useEffect, useRef, useState } from 'react'
import '@xterm/xterm/css/xterm.css'
import { Square, TerminalSquare } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { Button } from '@/components/ui/button'
import { PageState } from '@/components/app/PageState'
import { OrbitSpinner } from '@/components/ui/spinner'
import type { DesktopTerminalProfile } from '@/types/update'
import { useTerminalService, useTerminals } from './terminal-context'
import { createTerminalRenderer } from './terminal-renderer'

export function TaskTerminalView({
  sessionId,
  cwd,
  terminalId,
  onCreated,
  viewKey = 'task',
}: {
  sessionId: string
  cwd: string
  terminalId?: string
  onCreated?: (id: string) => void
  viewKey?: string
}) {
  const { t } = useI18n()
  const service = useTerminalService()
  const records = useTerminals(sessionId)
  const record = records.find((item) => item.id === terminalId)
  const exists = Boolean(record)
  // 进程已退出或刷新后连接已失效时回到创建页，不能停在一个无法输入、也无处新建的终端上。
  const ended = record?.status === 'exited'
  const endedNotice = !terminalId
    ? ''
    : !record
      ? t('terminal:workspace.ephemeral')
      : ended
        ? record.exitCode == null
          ? t('terminal:workspace.exited')
          : t('terminal:terminalPanel.processExited', { code: record.exitCode })
        : ''
  const [profiles, setProfiles] = useState<DesktopTerminalProfile[]>([])
  const [profileId, setProfileId] = useState('')
  const [error, setError] = useState('')
  const [creating, setCreating] = useState(false)
  const host = useRef<HTMLDivElement>(null)
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  useEffect(() => {
    let active = true
    if (service)
      void service.profiles().then(
        (items) => {
          if (active) {
            setProfiles(items)
            setProfileId(items.find((item) => item.default)?.id || items[0]?.id || '')
          }
        },
        (caught) => {
          if (active) setError(String(caught))
        },
      )
    return () => {
      active = false
    }
  }, [service])
  useEffect(() => {
    if (!service || !terminalId || !exists || ended || !host.current) return
    const renderer = service.renderer(`${viewKey}:${terminalId}`, () =>
      createTerminalRenderer(service.registry, terminalId),
    )
    renderer.attach(host.current, setError)
    return () => renderer.detach()
  }, [service, terminalId, viewKey, exists, ended])
  const create = async () => {
    if (!service || !profileId || creating) return
    setCreating(true)
    setError('')
    try {
      const id = await service.registry.create(sessionId, cwd, profileId)
      onCreated?.(id)
    } catch (caught) {
      if (mounted.current) setError(String(caught))
    } finally {
      if (mounted.current) setCreating(false)
    }
  }
  if (!service)
    return (
      <PageState
        kind="empty"
        title={t('terminal:workspace.unavailable')}
        description={t('terminal:workspace.nativeOnly')}
      />
    )
  if (!terminalId || !record || ended)
    return (
      <section className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-5 text-center">
        <span className="grid size-8 place-items-center rounded-md bg-key text-content-secondary shadow-key ring-1 ring-key-border">
          <TerminalSquare className="size-4" aria-hidden="true" />
        </span>
        <h2 className="text-sm font-medium">{t('terminal:workspace.create')}</h2>
        {endedNotice && <p className="text-xs text-content-muted">{endedNotice}</p>}
        <p className="max-w-full break-all font-mono text-xs text-content-muted">
          {cwd || t('terminal:workspace.defaultDirectory')}
        </p>
        <AppSelect
          className="w-auto max-w-full min-w-40 font-mono"
          aria-label={t('terminal:workspace.profile')}
          value={profileId}
          onChange={(event) => setProfileId(event.target.value)}
        >
          {profiles.map((profile) => (
            <option key={profile.id} value={profile.id}>
              {profile.label}
            </option>
          ))}
        </AppSelect>
        <Button disabled={!profileId || creating} onClick={() => void create()}>
          {t('terminal:workspace.create')}
        </Button>
        {error && (
          <p role="alert" className="text-sm text-danger">
            {error}
          </p>
        )}
      </section>
    )
  return (
    <section
      data-terminal-id={terminalId}
      className="flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
    >
      {/* 终端顶条：等宽目录 + 状态（运行中用品牌轨道指示）+ 停止键，下方是 --terminal-* 着色的凹陷终端井。 */}
      <div className="flex min-h-10 min-w-0 shrink-0 items-center gap-2 border-b border-stroke-soft py-1 pr-1.5 pl-3 text-xs">
        <span className="min-w-0 flex-1 truncate font-mono text-content-muted" title={record?.cwd}>
          {record?.cwd || t('terminal:workspace.defaultDirectory')}
        </span>
        {record?.orphaned && (
          <span className="text-warning-strong">{t('terminal:workspace.orphaned')}</span>
        )}
        {(record?.status === 'running' || record?.status === 'starting') && (
          <OrbitSpinner className="size-3.5" />
        )}
        <span
          className={
            record?.status === 'error'
              ? 'text-danger'
              : record?.status === 'running'
                ? 'font-medium text-accent-fg'
                : 'text-content-muted'
          }
        >
          {record?.status === 'exited'
            ? t('terminal:workspace.exited')
            : record?.status === 'error'
              ? t('terminal:workspace.error')
              : record?.status === 'starting'
                ? t('terminal:workspace.starting')
                : record?.status === 'stopping'
                  ? t('terminal:workspace.stopping')
                  : t('terminal:workspace.running')}
        </span>
        {record?.status !== 'exited' && (
          <Button
            variant="ghost"
            size="sm"
            className="hover:text-danger max-[650px]:min-h-11"
            disabled={record?.status === 'stopping'}
            onClick={() => {
              setError('')
              void service.registry.stop(terminalId).catch((caught) => setError(String(caught)))
            }}
          >
            <Square className="size-3" />
            {t('terminal:workspace.stop')}
          </Button>
        )}
      </div>
      <div
        ref={host}
        className="min-h-0 flex-1 overflow-hidden bg-[var(--terminal-bg)] px-2.5 py-1.5"
      />
      {(error || record?.error) && (
        <p role="alert" className="shrink-0 break-words p-2 text-xs text-danger">
          {error || record?.error}
        </p>
      )}
    </section>
  )
}
