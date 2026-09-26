// Provider 配置页数据 hook：加载配置 + 后台刷新模型目录，
// 提供启停/删除/配置更新等操作。页面不再维护 Provider 编辑草稿——
// 连接/模型走独立配置 API；工作台持有短期草稿，hook 管理已提交快照。
// 同文件导出 useProviderDiscovery（本地 Provider 扫描/导入）。
import { useCallback, useEffect, useRef, useState } from 'react'
import { apiJson } from '@/lib/api'
import { providerApi } from './provider-api'
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
  const [settingDefault, setSettingDefault] = useState('')
  const [settingModel, setSettingModel] = useState('')
  const configRevision = useRef(0)
  const mounted = useRef(false)

  // 首次加载配置，随后后台刷新各 Provider 的模型目录（结果回来后更新视图）。
  useEffect(() => {
    let active = true
    mounted.current = true
    const revision = configRevision.current
    apiJson<ConfigData>('/api/config')
      .then((data) => {
        if (!active) return undefined
        if (configRevision.current === revision) setConfig(data)
        setLoading(false)
        return apiJson<{ config?: ConfigData }>('/api/providers/models/refresh', {
          method: 'POST',
          body: '{}',
        })
      })
      .then((result) => {
        if (!active || configRevision.current !== revision || !result?.config) return
        setConfig(result.config)
      })
      .catch((caught: unknown) => {
        if (!active || configRevision.current !== revision) return
        setError(errorMessage(caught))
        setLoading(false)
      })
    return () => {
      active = false
      mounted.current = false
      configRevision.current += 1
    }
  }, [])

  // 配置更新统一入口：向导完成、视觉模型增删、策略保存后整份回写。
  const applyConfig = useCallback((data: ConfigData) => {
    // A delayed catalog response predates this explicit save and must not undo it.
    configRevision.current += 1
    setConfig(data)
    setLoading(false)
    setError('')
  }, [])

  // 自动接入响应可能早于刚保存的连接；重新读取而不是应用过期快照。
  const refreshConfig = useCallback(async () => {
    const revision = configRevision.current
    const data = await apiJson<ConfigData>('/api/config')
    if (mounted.current && configRevision.current === revision) applyConfig(data)
  }, [applyConfig])

  const toggleProvider = useCallback(
    async (provider: ProviderConfig, enabled: boolean) => {
      setToggling(provider.id)
      setError('')
      try {
        const updated = await apiJson<ConfigData>(
          `/api/providers/${encodeURIComponent(provider.id)}/enabled`,
          { method: 'PUT', body: JSON.stringify({ enabled }) },
        )
        applyConfig(updated)
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
    [applyConfig, notify, t],
  )

  const setDefaultProvider = useCallback(
    async (provider: ProviderConfig) => {
      setSettingDefault(provider.id)
      setError('')
      try {
        const updated = await apiJson<ConfigData>('/api/config', {
          method: 'PUT',
          body: JSON.stringify({ provider: provider.id, setAsDefault: true }),
        })
        applyConfig(updated)
        notify(t('config:configPage.defaultProviderUpdated', { name: provider.name }))
      } catch (caught) {
        setError(errorMessage(caught))
      } finally {
        setSettingDefault('')
      }
    },
    [applyConfig, notify, t],
  )

  const setProviderDefaultModel = useCallback(
    async (provider: ProviderConfig, model: string) => {
      setSettingModel(provider.id)
      setError('')
      try {
        const updated = await apiJson<ConfigData>('/api/config', {
          method: 'PUT',
          body: JSON.stringify({ provider: provider.id, model, setAsDefault: false }),
        })
        applyConfig(updated)
        notify(t('config:configPage.providerConnectionUpdated'))
      } catch (caught) {
        setError(errorMessage(caught))
      } finally {
        setSettingModel('')
      }
    },
    [applyConfig, notify, t],
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
        applyConfig(updated)
        notify(t('config:configPage.providerConnectionDeleted'))
      } catch (caught) {
        setError(errorMessage(caught))
      }
    },
    [applyConfig, notify, requestConfirm, t],
  )

  return {
    config,
    loading,
    error,
    toggling,
    settingDefault,
    settingModel,
    applyConfig,
    refreshConfig,
    setDefaultProvider,
    setProviderDefaultModel,
    toggleProvider,
    deleteProvider,
  }
}

type UseProviderDiscoveryOptions = {
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  onImported: (result: ProviderImportResult) => void
  onAutoImported: () => void | Promise<void>
  t: Translate
}

// Provider 发现 hook：扫描本地可导入 Provider、导入并回调结果，
// 处理扫描/导入中的错误与冲突确认。
export function useProviderDiscovery({
  requestConfirm,
  onImported,
  onAutoImported,
  t,
}: UseProviderDiscoveryOptions) {
  const [discovery, setDiscovery] = useState<DiscoveryData>({ providers: [], errors: [] })
  const [discovering, setDiscovering] = useState(true)
  const [error, setError] = useState('')
  const [operationError, setOperationError] = useState('')
  const [importing, setImporting] = useState('')
  const [autoImport, setAutoImport] = useState({ imported: 0, skipped: 0 })
  const refreshRevision = useRef(0)
  const onAutoImportedRef = useRef(onAutoImported)
  onAutoImportedRef.current = onAutoImported

  const refresh = useCallback(async () => {
    const revision = ++refreshRevision.current
    setDiscovering(true)
    setError('')
    try {
      const result = await providerApi.importLocal()
      if (revision !== refreshRevision.current) return
      setDiscovery(result.discovery)
      setAutoImport({ imported: result.imported.length, skipped: result.skipped.length })
      if (result.imported.length) await onAutoImportedRef.current()
    } catch (caught) {
      if (revision === refreshRevision.current) setError(errorMessage(caught))
    } finally {
      if (revision === refreshRevision.current) setDiscovering(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
    return () => {
      refreshRevision.current += 1
    }
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
    autoImport,
    discovering,
    error,
    operationError,
    importing,
    refresh,
    importProvider,
  }
}
