// 模型设置：连接列表/详情分栏；保留本地导入、快速向导及运行策略。
import { useEffect, useState } from 'react'
import { useLocation } from 'react-router-dom'
import { ChevronDown, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { pageStateStorage } from '@/lib/storage/page-state-storage'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { Button } from '@/components/ui/button'
import { ProviderWorkbench } from '@/features/config/components/provider/ProviderWorkbench'
import { ProviderConfigModal } from '@/features/config/components/provider/ProviderDialogs'
import { ProviderDiscovery } from '@/features/config/components/provider/ProviderDiscovery'
import { providerDiscoveryImportableCount } from '@/features/config/model/provider-discovery-state'
import { QuickSetupWizard } from '@/features/config/components/settings/QuickSetupWizard'
import { RuntimePolicySettings } from '@/features/config/components/settings/RuntimeSettings'
import { useProviderDiscovery, useProvidersConfig } from '@/features/config/hooks/useProvidersConfig'
import { VisualGenerationSettings } from '@/features/config/components/settings/VisualGenerationSettings'
import type { Notify } from '@/app/routes/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { ProviderConfig, ProviderType } from '@/features/config/model/config-types'

import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'

import { AppError, AppEmptyState } from '@/components/ui/app-primitives'

// 连接管理区展开状态持久化：用户显式展开后记住选择，否则保持折叠。
const MANAGE_CONNECTIONS_STORAGE_KEY = 'pisper.config.manageConnectionsOpen'

function storedManageOpen(): boolean | null {
  const stored = pageStateStorage.getItem(MANAGE_CONNECTIONS_STORAGE_KEY)
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
  const [selectedProviderId, setSelectedProviderId] = useState('')
  const [wizard, setWizard] = useState<WizardTarget | null>(null)
  const [cloningProvider, setCloningProvider] = useState<ProviderConfig | null>(null)
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
    onAutoImported: settings.refreshConfig,
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
  // 新增连接统一进入快速设置，协议与模型选项按步骤呈现。
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
  const defaultProvider = config.providers.find(
    (item) => item.id === defaultProviderId && item.configured,
  )
  const defaultModel = config.defaultModel || config.model
  // 扫描结果只提供轻提示，不覆盖用户的折叠偏好，也不自动展开管理区。
  const manageOpenEffective = manageOpen ?? false
  const importableCount = providerDiscoveryImportableCount(discovery.discovery)
  const setManageOpenPersisted = (open: boolean) => {
    setManageOpen(open)
    pageStateStorage.setItem(MANAGE_CONNECTIONS_STORAGE_KEY, open ? '1' : '0')
  }
  // 克隆需要保留来源连接的模型定义，继续复用完整连接弹窗。
  const openProviderClonerFor = (provider: ProviderConfig) => setCloningProvider(provider)

  return (
    <>
      {defaultProvider && defaultModel && (
        <div className="mb-4 text-sm">
          <p className="min-w-0 truncate text-muted-foreground">
            {t('config:configPage.currentChatModel')} ·{' '}
            <span className="text-foreground">
              {defaultProvider.name} / {defaultModel}
            </span>
          </p>
        </div>
      )}
      <p className="mb-4 flex items-center gap-2 text-xs text-muted-foreground" role="status">
        {discovery.discovering && <RefreshCw size={12} className="animate-spin" />}
        {discovery.discovering
          ? t('config:providerWorkbench.scanningLocal')
          : t('config:providerWorkbench.localImportStatus', {
              imported: discovery.autoImport.imported,
              skipped: discovery.autoImport.skipped,
            })}
      </p>
      <ProviderWorkbench
        config={config}
        selectedProviderId={selectedProviderId}
        onSelectProvider={setSelectedProviderId}
        toggling={settings.toggling}
        settingDefault={settings.settingDefault}
        settingModel={settings.settingModel}
        onSave={(data) => {
          settings.applyConfig(data)
          notify(t('config:configPage.providerConnectionUpdated'))
        }}
        onClone={openProviderClonerFor}
        onDelete={settings.deleteProvider}
        deletingModel={settings.deletingModel}
        onDeleteModel={settings.deleteModel}
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
              if (provider) {
                setSelectedProviderId(provider.id)
                document
                  .querySelector('[data-model-provider-split-panel]')
                  ?.scrollIntoView({ block: 'start', behavior: 'smooth' })
              }
            }}
          />
        </CollapsibleContent>
      </Collapsible>
      {wizard && (
        <QuickSetupWizard
          config={config}
          providerType={wizard.providerType || 'chat'}
          initialProviderId={wizard.providerId}
          onConfigChanged={(data) => {
            settings.applyConfig(data)
            if (data.createdProviderId) setSelectedProviderId(data.createdProviderId)
          }}
          onClose={() => setWizard(null)}
          onCompleted={(data) => {
            settings.applyConfig(data)
            if (data.createdProviderId) setSelectedProviderId(data.createdProviderId)
            notify(t('config:configPage.setupComplete'))
            setWizard(null)
          }}
        />
      )}
      {cloningProvider && (
        <ProviderConfigModal
          initialProviderType={cloningProvider.type}
          cloneProvider={cloningProvider}
          onClose={() => setCloningProvider(null)}
          onCreated={(data) => {
            settings.applyConfig(data)
            if (data.createdProviderId) setSelectedProviderId(data.createdProviderId)
            notify(t('config:configPage.providerConnectionCreated'))
            setCloningProvider(null)
          }}
        />
      )}
    </>
  )
}
