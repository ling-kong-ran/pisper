// 模型设置：ZCode 式连接列表/详情分栏；保留本地导入、快速向导及运行策略。
import { useEffect, useState } from 'react'
import { useLocation } from 'react-router-dom'
import { ChevronDown, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { Button } from '@/components/ui/button'
import { ProviderWorkbench } from './ProviderWorkbench'
import { ProviderConfigModal } from './ProviderDialogs'
import { ProviderDiscovery } from './ProviderDiscovery'
import { providerDiscoveryImportableCount } from './provider-discovery-state'
import { QuickSetupWizard } from './QuickSetupWizard'
import { RuntimePolicySettings } from './RuntimeSettings'
import { useProviderDiscovery, useProvidersConfig } from './useProvidersConfig'
import { VisualGenerationSettings } from './VisualGenerationSettings'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { ProviderConfig, ProviderType } from './config-types'

import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'

import { AppError, AppEmptyState } from '@/components/ui/app-primitives'

// 连接管理区展开状态持久化：用户显式展开后记住选择，否则保持折叠。
const MANAGE_CONNECTIONS_STORAGE_KEY = 'pisper.config.manageConnectionsOpen'

function storedManageOpen(): boolean | null {
  const stored = window.localStorage.getItem(MANAGE_CONNECTIONS_STORAGE_KEY)
  return stored === '1' ? true : stored === '0' ? false : null
}

type ModelsSettingsProps = {
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

// 向导打开参数：对话/视觉共用三步连接配置，不再从预设 Provider 开始。
type WizardTarget = {
  providerId?: string
  providerType?: ProviderType
}

export function ModelsSettings({
  notify,
  registerPrimaryAction,
  requestConfirm,
}: ModelsSettingsProps) {
  const { t } = useI18n()
  const location = useLocation()
  const importRequested = new URLSearchParams(location.search).get('import') === '1'
  const [wizard, setWizard] = useState<WizardTarget | null>(null)
  // 连接弹窗按需新建或编辑；视觉连接也必须能修改 Key、URL 和模型定义。
  const [providerModal, setProviderModal] = useState<{
    providerType: ProviderType
    provider?: ProviderConfig
    cloneProvider?: ProviderConfig
  } | null>(null)
  const [manageOpen, setManageOpen] = useState<boolean | null>(() =>
    importRequested ? true : storedManageOpen(),
  )
  useEffect(() => {
    if (importRequested) setManageOpen(true)
  }, [importRequested])
  const settings = useProvidersConfig({ notify, requestConfirm, t })
  const { config } = settings
  const discovery = useProviderDiscovery({
    requestConfirm,
    onImported: (result) => {
      settings.applyConfig(result.config)
      const imported = result.config.providers.find((item) => item.id === result.providerId)
      notify(
        result.kind === 'authentication'
          ? t('config:configPage.nameLoginStateHasBeenLoadedIntoPisper', {
              name: imported?.name || result.providerId,
            })
          : t('config:configPage.nameConfigurationHasBeenLoadedIntoPisper', {
              name: imported?.name || result.providerId,
            }),
      )
    },
    t,
  })
  // 页面主操作 = 快速配置向导（三步完成对话模型配置）。
  usePagePrimaryAction(registerPrimaryAction, () => setWizard({ providerType: 'chat' }))

  if (!config) {
    return (
      <AppEmptyState>
        <RefreshCw className="animate-spin" size={24} />
        <h2>{t('config:configPage.loadingModelCatalog')}</h2>
        <p>{t('config:configPage.readingProvidersAndAuthenticationStatus')}</p>
        {settings.error && <AppError>{settings.error}</AppError>}
      </AppEmptyState>
    )
  }

  const defaultProviderId = config.defaultProvider || config.provider
  const defaultProvider = config.providers.find((item) => item.id === defaultProviderId)
  // 扫描结果只提供轻提示，不覆盖用户的折叠偏好，也不自动展开管理区。
  const manageOpenEffective = manageOpen ?? false
  const importableCount = providerDiscoveryImportableCount(discovery.discovery)
  const setManageOpenPersisted = (open: boolean) => {
    setManageOpen(open)
    window.localStorage.setItem(MANAGE_CONNECTIONS_STORAGE_KEY, open ? '1' : '0')
  }
  // 从列表进入连接编辑弹窗；摘要卡仍进入向导以便直接切换默认模型。
  const openProviderClonerFor = (provider: ProviderConfig) =>
    setProviderModal({ providerType: provider.type, cloneProvider: provider })
  const openWizardFor = (provider: ProviderConfig) =>
    setWizard({ providerId: provider.id, providerType: provider.type })

  return (
    <>
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3 text-sm">
        <p className="min-w-0 truncate text-muted-foreground">
          {t('config:configPage.currentChatModel')} ·{' '}
          <span className="text-foreground">
            {defaultProvider?.name || '—'} / {config.defaultModel || config.model || '—'}
          </span>
        </p>
        <Button variant="outline" size="sm" onClick={() => setWizard({ providerType: 'chat' })}>
          {t('config:configPage.quickSetup')}
        </Button>
      </div>
      <ProviderWorkbench
        config={config}
        toggling={settings.toggling}
        settingDefault={settings.settingDefault}
        settingModel={settings.settingModel}
        onSave={(data) => {
          settings.applyConfig(data)
          notify(t('config:configPage.providerConnectionUpdated'))
        }}
        onAdd={() => setProviderModal({ providerType: 'chat' })}
        onQuickSetup={(provider) =>
          provider ? openWizardFor(provider) : setWizard({ providerType: 'chat' })
        }
        onClone={openProviderClonerFor}
        onDelete={settings.deleteProvider}
        onToggle={settings.toggleProvider}
        onSetDefault={settings.setDefaultProvider}
        onSetDefaultModel={settings.setProviderDefaultModel}
      />
      {settings.error && <AppError>{settings.error}</AppError>}
      {importableCount > 0 && (
        <div className="flex flex-wrap items-center gap-x-2 gap-y-0 text-[length:var(--app-small-size)] text-[var(--text-muted)]">
          <span>{t('config:configPage.localProviderImportHint', { count: importableCount })}</span>
          <Button
            type="button"
            variant="link"
            size="sm"
            className="h-auto min-h-11 shrink-0 px-1 py-0 font-normal text-[var(--text-secondary)] sm:min-h-8"
            aria-expanded={manageOpenEffective}
            aria-controls="model-connection-management"
            onClick={() => setManageOpenPersisted(true)}
          >
            {t('config:configPage.reviewLocalProviders')}
          </Button>
        </div>
      )}
      <Collapsible
        open={manageOpenEffective}
        onOpenChange={setManageOpenPersisted}
        data-config-card="models-advanced"
      >
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className="group flex w-full cursor-pointer items-center gap-[7px] [margin:2px_0_10px] border-0 bg-transparent p-[4px_2px] text-left"
          >
            <ChevronDown
              size={15}
              className="shrink-0 text-[var(--text-muted)] transition-transform group-data-[state=closed]:-rotate-90"
            />
            <span className="shrink-0 text-[13px] font-[700] text-[var(--text-secondary)]">
              {t('config:providerWorkbench.advanced')}
            </span>
            <span className="min-w-0 text-[12px] text-[var(--text-tertiary)]">
              {t('config:providerWorkbench.advancedHint')}
            </span>
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent id="model-connection-management">
          <ProviderDiscovery
            discovery={discovery.discovery}
            discovering={discovery.discovering}
            error={discovery.error || discovery.operationError}
            importing={discovery.importing}
            forceVisible={importRequested}
            onRefresh={discovery.refresh}
            onImport={discovery.importProvider}
          />
          <div className="[margin-top:12px]">
            <RuntimePolicySettings
              config={config}
              notify={notify}
              onConfigChanged={settings.applyConfig}
            />
          </div>
        </CollapsibleContent>
      </Collapsible>
      <VisualGenerationSettings
        config={config}
        notify={notify}
        toggling={settings.toggling}
        onToggleProvider={settings.toggleProvider}
        onCloneProvider={openProviderClonerFor}
        onDeleteProvider={settings.deleteProvider}
        onQuickSetup={() => setWizard({ providerType: 'visual' })}
        onEditVisualProvider={(providerId) => {
          const provider = config.providers.find((item) => item.id === providerId)
          if (provider) setProviderModal({ providerType: 'visual', provider })
        }}
      />
      {wizard && (
        <QuickSetupWizard
          config={config}
          providerType={wizard.providerType || 'chat'}
          initialProviderId={wizard.providerId}
          onClose={() => setWizard(null)}
          onCompleted={(data) => {
            settings.applyConfig(data)
            notify(t('config:configPage.setupComplete'))
            setWizard(null)
          }}
        />
      )}
      {providerModal && (
        <ProviderConfigModal
          initialProviderType={providerModal.providerType}
          initialProvider={providerModal.provider}
          cloneProvider={providerModal.cloneProvider}
          onConfigChanged={settings.applyConfig}
          onClose={() => setProviderModal(null)}
          onCreated={(data) => {
            settings.applyConfig(data)
            if (providerModal.provider) notify(t('config:configPage.providerConnectionUpdated'))
            else notify(t('config:configPage.providerConnectionCreated'))
            setProviderModal(null)
          }}
        />
      )}
    </>
  )
}
