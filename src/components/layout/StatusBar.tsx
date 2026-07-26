import { useCallback, useEffect, useRef, useState } from 'react'
import { Bot } from 'lucide-react'
import { STORAGE_KEYS } from '@/app/storage'
import { useI18n } from '@/app/use-i18n'
import { ACTIVE_SESSION_CHANGED_EVENT, SESSIONS_UPDATED_EVENT } from '@/features/chat/events'
import { apiJson } from '@/lib/api'
import { formatTokenCount } from '@/lib/format'

const USAGE_UPDATED_EVENT = 'vesper:usage-updated'

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
    <footer className="status-bar">
      <span className="status-model">
        <Bot size={12} />
        {modelLabel || t('navigation:statusBar.noModelConfigured')}
      </span>
      <span className="status-usage">
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
