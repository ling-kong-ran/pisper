// 底部状态栏：展示活动会话模型/上下文用量（按需轮询与事件刷新）、
// 插件启用统计与项目外链；点击打开对应设置页。
// 运行时连接态：复用「今日用量」轮询作为健康探针——在线时 15s 一次，
// 判定离线后缩短为 3s 快速重试；仅网络不可达（无 HTTP 响应）判为断开，
// 收到 HTTP 状态码（含错误码）说明链路仍可达。断开时连接点变琥珀色并
// 显示「重连中」，恢复后回到正常态；断开/恢复各提示一次（10s 内同状态节流）。
import { useCallback, useEffect, useRef, useState } from 'react'
import { Bot } from 'lucide-react'
import { STORAGE_KEYS } from '@/app/storage'
import { useI18n } from '@/app/use-i18n'
import { ACTIVE_SESSION_CHANGED_EVENT, SESSIONS_UPDATED_EVENT } from '@/features/chat/events'
import { apiJson } from '@/lib/api'
import { ApiError } from '@/lib/http'
import { formatTokenCount, normalizeTokenUsage, type TokenUsage } from '@/lib/format'
import { AppToast, type ToastTone } from '@/components/ui/toast'
import { cn } from '@/lib/utils'

const USAGE_UPDATED_EVENT = 'pisper:usage-updated'
// 在线时的常规轮询间隔与离线后的快速重试间隔。
const ONLINE_POLL_INTERVAL_MS = 15_000
const OFFLINE_RETRY_INTERVAL_MS = 3_000
// 同一连接状态的 toast 节流窗口：10s 内不重复提示。
const CONNECTION_TOAST_THROTTLE_MS = 10_000

type PluginStats = {
  enabled: number
  total: number
}

type ConnectionToast = {
  id: number
  message: string
  tone: ToastTone
}

type StatusBarProps = {
  page: string
  pluginStats: PluginStats | null
}

export function StatusBar({ page, pluginStats }: StatusBarProps) {
  const { t, language } = useI18n()
  const [usage, setUsage] = useState<TokenUsage | null>(null)
  const [modelLabel, setModelLabel] = useState('')
  const [runtimeOnline, setRuntimeOnline] = useState(true)
  const [connectionToast, setConnectionToast] = useState<ConnectionToast | null>(null)
  const runtimeOnlineRef = useRef(true)
  const lastConnectionToast = useRef<{ online: boolean; at: number } | null>(null)
  const connectionToastSequence = useRef(0)
  const modelRequest = useRef(0)

  // 健康探针：拉取今日用量。成功或收到 HTTP 响应（含错误码）视为在线，
  // 仅网络层失败/超时（ApiError 无 status）判定为连接断开。
  const probeRuntime = useCallback(async (): Promise<boolean> => {
    try {
      setUsage(normalizeTokenUsage(await apiJson<unknown>('/api/usage/today')))
      return true
    } catch (error) {
      return error instanceof ApiError && error.status != null
    }
  }, [])

  // 连接态切换：更新状态并触发一次 toast（同状态 10s 内节流）。
  const applyRuntimeOnline = useCallback(
    (online: boolean) => {
      if (runtimeOnlineRef.current === online) return
      runtimeOnlineRef.current = online
      setRuntimeOnline(online)
      const now = Date.now()
      const last = lastConnectionToast.current
      if (last && last.online === online && now - last.at < CONNECTION_TOAST_THROTTLE_MS) return
      lastConnectionToast.current = { online, at: now }
      connectionToastSequence.current += 1
      setConnectionToast({
        id: connectionToastSequence.current,
        message: online
          ? t('common:statusBar.connectionRestored')
          : t('common:statusBar.connectionLost'),
        tone: online ? 'success' : 'error',
      })
    },
    [t],
  )

  // 连接探测循环：自适应调度（在线 15s / 离线 3s），可见性恢复时立即探测。
  useEffect(() => {
    let active = true
    let timer = 0
    const run = async () => {
      const online = await probeRuntime()
      if (!active) return
      applyRuntimeOnline(online)
      timer = window.setTimeout(
        () => void run(),
        online ? ONLINE_POLL_INTERVAL_MS : OFFLINE_RETRY_INTERVAL_MS,
      )
    }
    void run()
    const refreshWhenVisible = () => {
      if (document.visibilityState !== 'visible') return
      // 回到前台立即探测一次：清掉排队中的旧轮询，避免出现双循环。
      window.clearTimeout(timer)
      void run()
    }
    document.addEventListener('visibilitychange', refreshWhenVisible)
    return () => {
      active = false
      window.clearTimeout(timer)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
    }
  }, [applyRuntimeOnline, probeRuntime])

  // 用量事件推送：只刷新展示数据，不参与连接态判定（失败静默）。
  const refreshUsageSilently = useCallback(async () => {
    try {
      setUsage(normalizeTokenUsage(await apiJson<unknown>('/api/usage/today')))
    } catch {}
  }, [])

  useEffect(() => {
    window.addEventListener(USAGE_UPDATED_EVENT, refreshUsageSilently)
    return () => window.removeEventListener(USAGE_UPDATED_EVENT, refreshUsageSilently)
  }, [refreshUsageSilently])

  // 刷新模型标签：并发保护（请求序号），会话模型未解析为 unknown 时按未配置处理。
  const refreshModel = useCallback(
    async (sessionId = localStorage.getItem(STORAGE_KEYS.activeSession) || '') => {
      const request = ++modelRequest.current
      try {
        const [config, sessionData] = await Promise.all([
          apiJson<{ provider?: string; model?: string }>('/api/config'),
          apiJson<{
            sessions?: Array<{ id: string; model?: string }>
          }>('/api/sessions'),
        ])
        if (request !== modelRequest.current) return
        const session = sessionData.sessions?.find((item: { id: string }) => item.id === sessionId)
        const label = session?.model || (config.model ? `${config.provider}/${config.model}` : '')
        // 会话尚未解析出真实模型时可能是 "unknown/unknown"，按未配置处理
        setModelLabel(/(^|\/)unknown$/i.test(label) ? '' : label)
      } catch {}
    },
    [],
  )

  useEffect(() => {
    const syncModel = (event: Event) => {
      const detail = (event as CustomEvent<{ id?: string; model?: string }>).detail
      const sessionId = detail?.id || localStorage.getItem(STORAGE_KEYS.activeSession) || ''
      if (detail?.model) {
        modelRequest.current += 1
        setModelLabel(/(^|\/)unknown$/i.test(detail.model) ? '' : detail.model)
      } else {
        void refreshModel(sessionId)
      }
    }
    const refreshFromSessions = () => {
      void refreshModel()
    }
    void refreshModel()
    window.addEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncModel)
    window.addEventListener(SESSIONS_UPDATED_EVENT, refreshFromSessions)
    return () => {
      window.removeEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncModel)
      window.removeEventListener(SESSIONS_UPDATED_EVENT, refreshFromSessions)
    }
  }, [refreshModel])

  const usageTitle = usage
    ? t('navigation:statusBar.inputInputOutputOutputReasoningReasoningCacheReadCacheRead', {
        input: usage.input.toLocaleString(language),
        output: usage.output.toLocaleString(language),
        reasoning: usage.reasoning.toLocaleString(language),
        cacheRead: usage.cacheRead.toLocaleString(language),
      })
    : t('navigation:statusBar.calculatingTodaySTokenUsage')

  const connectionTitle = runtimeOnline
    ? t('navigation:statusBar.connected')
    : t('common:statusBar.reconnecting')

  return (
    <footer className="status-bar [&_em]:text-[var(--star-strong)] [&_em]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_em]:[font-style:normal] [&_em]:font-[700] [&_em::before]:[content:'✦'] [&_em::before]:mr-[4px] [&_em::before]:text-[var(--star)] [&_em::before]:text-[9px] [&_em.amber]:text-[var(--text-muted)] [&_em.amber]:font-[inherit] [&_em.amber::before]:[content:none] max-[900px]:hidden flex h-[28px] flex-none items-center justify-between gap-[12px] [border-top:1px_solid_var(--stroke)] bg-[var(--sidebar-bg)] [padding:0_14px] text-[var(--text-muted)] text-[11px]">
      <span className="status-model [.status-bar_&]:flex [.status-bar_&]:min-w-0 [.status-bar_&]:items-center [.status-bar_&]:gap-[6px] [.status-bar_&]:overflow-hidden [.status-bar_&]:text-ellipsis [.status-bar_&]:whitespace-nowrap">
        <span
          aria-hidden
          title={connectionTitle}
          className={cn(
            'size-[7px] flex-none rounded-full',
            runtimeOnline ? 'bg-[var(--success)]' : 'animate-pulse bg-[var(--warning-strong)]',
          )}
        />
        <Bot size={12} />
        {modelLabel || t('navigation:statusBar.noModelConfigured')}
      </span>
      <span className="status-usage [.status-bar_&]:flex [.status-bar_&]:flex-none [.status-bar_&]:items-center [.status-bar_&]:gap-[6px]">
        {['skills', 'mcp', 'workflows', 'workflowCreate'].includes(page) ? (
          <>
            {t('navigation:statusBar.nativeRuntime')}{' '}
            {runtimeOnline ? (
              <em>{t('navigation:statusBar.connected')}</em>
            ) : (
              <span className="font-[700] text-[var(--warning-strong)]">
                {t('common:statusBar.reconnecting')}
              </span>
            )}
          </>
        ) : page === 'plugins' ? (
          t('navigation:statusBar.enabledTotalEnabled', {
            enabled: pluginStats?.enabled ?? '—',
            total: pluginStats?.total ?? '—',
          })
        ) : (
          <>
            {t('navigation:statusBar.todaySTokens')}{' '}
            <em title={usageTitle}>{usage ? formatTokenCount(usage.totalTokens) : '—'}</em>
            {!runtimeOnline && (
              <span className="font-[700] text-[var(--warning-strong)]">
                {t('common:statusBar.reconnecting')}
              </span>
            )}
          </>
        )}
      </span>
      {connectionToast && (
        <AppToast
          key={connectionToast.id}
          open
          message={connectionToast.message}
          tone={connectionToast.tone}
          closeLabel={t('common:ui.closeDialog')}
          onOpenChange={(open) => !open && setConnectionToast(null)}
        />
      )}
    </footer>
  )
}
