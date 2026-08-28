// Provider 配置页数据 hook：加载配置 + 后台刷新模型目录，
// 提供启停/删除/配置更新等操作。页面不再维护 Provider 编辑草稿——
// 配置改动全部走快速配置向导或视觉生成卡，完成后整份配置回写。
// 同文件导出 useProviderDiscovery（本地 Provider 扫描/导入）。
import { useCallback, useEffect, useState } from 'react'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type {
  ConfigData,
  DiscoveredProvider,
  DiscoveryData,
  ProviderConfig,
  ProviderImportResult,
  Translate,
} from './config-types'

// 归一化异常为文案（配置页共用）。
function errorMessage(caught: unknown) {
  return caught instanceof Error ? caught.message : String(caught)
}

type UseProvidersConfigOptions = {
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  t: Translate
}

export function useProvidersConfig({ notify, requestConfirm, t }: UseProvidersConfigOptions) {
  const [config, setConfig] = useState<ConfigData | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [toggling, setToggling] = useState('')

  // 首次加载配置，随后后台刷新各 Provider 的模型目录（结果回来后更新视图）。
  useEffect(() => {
    let active = true
    apiJson<ConfigData>('/api/config')
      .then((data) => {
        if (!active) return undefined
        setConfig(data)
        setLoading(false)
        return apiJson<{ config?: ConfigData }>('/api/providers/models/refresh', {
          method: 'POST',
          body: '{}',
        })
      })
      .then((result) => {
        if (!active || !result?.config) return
        setConfig(result.config)
      })
      .catch((caught: unknown) => {
        if (!active) return
        setError(errorMessage(caught))
        setLoading(false)
      })
    return () => {
      active = false
    }
  }, [])

  // 配置更新统一入口：向导完成、视觉模型增删、策略保存后整份回写。
  const applyConfig = useCallback((data: ConfigData) => {
    setConfig(data)
    setError('')
  }, [])

  const toggleProvider = useCallback(
    async (provider: ProviderConfig, enabled: boolean) => {
      setToggling(provider.id)
      setError('')
      try {
        const updated = await apiJson<ConfigData>(
          `/api/providers/${encodeURIComponent(provider.id)}/enabled`,
          { method: 'PUT', body: JSON.stringify({ enabled }) },
        )
        setConfig(updated)
        notify(
          t('config:configPage.nameState', {
            name: provider.name,
            state: enabled ? t('config:configPage.enabled') : t('config:configPage.disabled'),
          }),
        )
      } catch (caught) {
        setError(errorMessage(caught))
      } finally {
        setToggling('')
      }
    },
    [notify, t],
  )

  const deleteProvider = useCallback(
    async (provider: ProviderConfig) => {
      const approved = await requestConfirm({
        title: t('config:configPage.deleteProviderConnection'),
        message: t(
          'config:configPage.deleteNameItsModelSettingsAndAuthenticationDetailsWillAlsoBeRemoved',
          { name: provider.name },
        ),
        confirmLabel: t('config:configPage.delete'),
      })
      if (!approved) return
      setError('')
      try {
        const updated = await apiJson<ConfigData>(
          `/api/providers/${encodeURIComponent(provider.id)}`,
          { method: 'DELETE' },
        )
        setConfig(updated)
        notify(t('config:configPage.providerConnectionDeleted'))
      } catch (caught) {
        setError(errorMessage(caught))
      }
    },
    [notify, requestConfirm, t],
  )

  return {
    config,
    loading,
    error,
    toggling,
    applyConfig,
    toggleProvider,
    deleteProvider,
  }
}

type UseProviderDiscoveryOptions = {
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  onImported: (result: ProviderImportResult) => void
  t: Translate
}

// Provider 发现 hook：扫描本地可导入 Provider、导入并回调结果，
// 处理扫描/导入中的错误与冲突确认。
export function useProviderDiscovery({
  requestConfirm,
  onImported,
  t,
}: UseProviderDiscoveryOptions) {
  const [discovery, setDiscovery] = useState<DiscoveryData>({ providers: [], errors: [] })
  const [discovering, setDiscovering] = useState(true)
  const [error, setError] = useState('')
  const [operationError, setOperationError] = useState('')
  const [importing, setImporting] = useState('')

  const refresh = useCallback(async () => {
    setDiscovering(true)
    setError('')
    try {
      setDiscovery(await apiJson<DiscoveryData>('/api/providers/discovery'))
    } catch (caught) {
      setError(errorMessage(caught))
    } finally {
      setDiscovering(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const importProvider = useCallback(
    async (provider: DiscoveredProvider) => {
      const source =
        provider.source === 'codex-config'
          ? 'Codex config.toml'
          : provider.source === 'claude-config'
            ? 'Claude settings.json'
            : provider.source === 'codex-auth'
              ? 'Codex login'
              : 'Claude login'
      const authentication = provider.kind === 'authentication'
      const approved = await requestConfirm({
        title: authentication
          ? t('config:configPage.loadLoginState')
          : t('config:configPage.loadProviderConfiguration'),
        message: authentication
          ? t('config:configPage.loadOfficialProviderLoginStateFromSource', { source })
          : t('config:configPage.loadThisProviderConfigurationFromSource', { source }),
        confirmLabel: authentication
          ? t('config:configPage.loadLoginState')
          : t('config:configPage.loadConfiguration'),
      })
      if (!approved) return
      setImporting(provider.id)
      setOperationError('')
      try {
        const result = await apiJson<ProviderImportResult>(
          `/api/providers/${encodeURIComponent(provider.id)}/import`,
          { method: 'POST', body: '{}' },
        )
        setDiscovery(result.discovery)
        onImported(result)
      } catch (caught) {
        setOperationError(errorMessage(caught))
      } finally {
        setImporting('')
      }
    },
    [onImported, requestConfirm, t],
  )

  return {
    discovery,
    discovering,
    error,
    operationError,
    importing,
    refresh,
    importProvider,
  }
}
