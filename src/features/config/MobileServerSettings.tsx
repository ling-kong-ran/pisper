// 移动端「服务器」设置页：列出壳内已配对的桌面端，支持切换/删除，
// 以及跳回连接页添加新服务器。数据来自移动端壳（Tauri 命令），
// 不走 runtime——配对与令牌管理始终在壳内。
import { useCallback, useEffect, useState } from 'react'
import { MonitorSmartphone, Plus, Server, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { SettingsCard as Panel } from './settings-primitives'
import { Button } from '@/components/ui/button'

type ServerItem = {
  id: string
  name: string
  endpoints: Array<{ t: string; url: string }>
  pairedAt: string
}

type MobileState = {
  paired: boolean
  proxyUrl: string
  activeId: string | null
  servers: ServerItem[]
}

function invokeMobile<T>(command: string, args?: unknown): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke
  if (!invoke) return Promise.reject(new Error('native bridge unavailable'))
  return invoke<T>(command, args)
}

export function MobileServerSettings() {
  const { t } = useI18n()
  const [state, setState] = useState<MobileState | null>(null)
  const [error, setError] = useState('')
  const [busyId, setBusyId] = useState('')

  const refresh = useCallback(async () => {
    try {
      setState(await invokeMobile<MobileState>('mobile_state'))
      setError('')
    } catch {
      setError(t('config:mobileServer.loadFailed'))
    }
  }, [t])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const run = async (command: string, args: unknown, id = '') => {
    setBusyId(id || command)
    try {
      setState(await invokeMobile<MobileState>(command, args))
      setError('')
    } catch (cause) {
      setError(String(cause))
    } finally {
      setBusyId('')
    }
  }

  const addServer = async () => {
    try {
      // 跳回壳内连接页（扫码/手动配对）。
      const url = await invokeMobile<string>('mobile_connect_url')
      if (url) window.location.href = url
    } catch (cause) {
      setError(String(cause))
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <Panel className="flex flex-col gap-3 p-4">
        <div className="flex items-center justify-between gap-3">
          <div className="flex items-start gap-[11px]">
            <span className="grid size-[38px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--star-soft)] text-[var(--star-strong)]">
              <MonitorSmartphone size={19} />
            </span>
            <div>
              <h2 className="text-[16px]">{t('config:mobileServer.title')}</h2>
              <p className="mt-1 text-[13px] leading-[1.55] text-[var(--text-muted)]">
                {t('config:mobileServer.description')}
              </p>
            </div>
          </div>
          <Button size="sm" onClick={() => void addServer()}>
            <Plus size={14} />
            {t('config:mobileServer.addServer')}
          </Button>
        </div>
        {error ? <p className="text-[12px] text-[var(--danger)]">{error}</p> : null}
      </Panel>

      <Panel className="flex flex-col gap-3 p-4">
        {state?.servers?.length ? (
          <ul className="flex flex-col gap-2">
            {state.servers.map((server) => {
              const active = server.id === state.activeId
              return (
                <li
                  key={server.id}
                  className="flex items-center justify-between gap-3 rounded-[var(--r-sm)] border border-[var(--border)] px-3 py-2"
                >
                  <div className="flex min-w-0 items-center gap-2">
                    <Server size={15} className="flex-none text-[var(--text-muted)]" />
                    <div className="min-w-0">
                      <div className="truncate text-[13px]">
                        {server.name}
                        {active ? (
                          <span className="ml-2 text-[11px] text-[var(--star-strong)]">
                            {t('config:mobileServer.current')}
                          </span>
                        ) : null}
                      </div>
                      <div className="truncate text-[11px] text-[var(--text-muted)]">
                        {server.endpoints[0]?.url}
                      </div>
                    </div>
                  </div>
                  <div className="flex flex-none items-center gap-1">
                    {!active ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        disabled={busyId === server.id}
                        onClick={() =>
                          void run('mobile_select_server', { id: server.id }, server.id)
                        }
                      >
                        {t('config:mobileServer.connect')}
                      </Button>
                    ) : null}
                    <Button
                      size="sm"
                      variant="ghost"
                      aria-label={t('config:mobileServer.forget')}
                      onClick={() => {
                        if (window.confirm(t('config:mobileServer.forgetConfirm'))) {
                          void run('mobile_forget_server', { id: server.id }, server.id)
                        }
                      }}
                    >
                      <Trash2 size={14} />
                    </Button>
                  </div>
                </li>
              )
            })}
          </ul>
        ) : (
          <p className="text-[12px] text-[var(--text-muted)]">{t('config:mobileServer.empty')}</p>
        )}
      </Panel>
    </div>
  )
}
