// 底部状态栏：展示活动会话模型/上下文用量（按需轮询与事件刷新）、
// 插件启用统计与项目外链；点击打开对应设置页。
import { useCallback, useEffect, useRef, useState } from 'react'
import { Bot } from 'lucide-react'
import { STORAGE_KEYS } from '@/app/storage'
import { useI18n } from '@/app/use-i18n'
import { ACTIVE_SESSION_CHANGED_EVENT, SESSIONS_UPDATED_EVENT } from '@/features/chat/events'
import { apiJson } from '@/lib/api'
import { formatTokenCount } from '@/lib/format'

const USAGE_UPDATED_EVENT = 'pisper:usage-updated'

type Usage = {
  input: number
  output: number
  reasoning: number
  cacheRead: number
  totalTokens: number
}

type PluginStats = {
  enabled: number
  total: number
}

type StatusBarProps = {
  page: string
  pluginStats: PluginStats | null
}

export function StatusBar({ page, pluginStats }: StatusBarProps) {
  const { t, language } = useI18n()
  const [usage, setUsage] = useState<Usage | null>(null)
  const [modelLabel, setModelLabel] = useState('')
  const modelRequest = useRef(0)

  // 刷新今日用量：轮询 + 可见性恢复 + 事件推送三重刷新，失败静默。
  const refreshUsage = useCallback(async () => {
    try {
      setUsage(await apiJson<Usage>('/api/usage/today'))
    } catch {}
  }, [])

  useEffect(() => {
    refreshUsage()
    const timer = window.setInterval(refreshUsage, 15_000)
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') refreshUsage()
    }
    document.addEventListener('visibilitychange', refreshWhenVisible)
    window.addEventListener(USAGE_UPDATED_EVENT, refreshUsage)
    return () => {
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
      window.removeEventListener(USAGE_UPDATED_EVENT, refreshUsage)
    }
  }, [refreshUsage])

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

  return (
    <footer className="status-bar [&_em]:text-[var(--star-strong)] [&_em]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_em]:[font-style:normal] [&_em]:font-[700] [&_em::before]:[content:'✦'] [&_em::before]:mr-[4px] [&_em::before]:text-[var(--star)] [&_em::before]:text-[9px] [&_em.amber]:text-[var(--text-muted)] [&_em.amber]:font-[inherit] [&_em.amber::before]:[content:none] max-[900px]:hidden flex h-[28px] flex-none items-center justify-between gap-[12px] [border-top:1px_solid_var(--stroke)] bg-[var(--sidebar-bg)] [padding:0_14px] text-[var(--text-muted)] text-[11px]">
      <span className="status-model [.status-bar_&]:flex [.status-bar_&]:min-w-0 [.status-bar_&]:items-center [.status-bar_&]:gap-[6px] [.status-bar_&]:overflow-hidden [.status-bar_&]:text-ellipsis [.status-bar_&]:whitespace-nowrap">
        <Bot size={12} />
        {modelLabel || t('navigation:statusBar.noModelConfigured')}
      </span>
      <span className="status-usage [.status-bar_&]:flex [.status-bar_&]:flex-none [.status-bar_&]:items-center [.status-bar_&]:gap-[6px]">
        {['skills', 'mcp', 'workflows', 'workflowCreate'].includes(page) ? (
          <>
            {t('navigation:statusBar.nativeRuntime')} <em>{t('navigation:statusBar.connected')}</em>
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
          </>
        )}
      </span>
    </footer>
  )
}
