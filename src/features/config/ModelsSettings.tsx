// 设置 › 模型（方向 A，效果图 desktop-settings-light.png、样稿 settings.html）：页标题下面是默认值三行
// （默认模型、思考强度、可用工具，选中即保存），然后是“提供方”分区（每个提供方一行，原位展开编辑连接与模型，
// 末尾“添加提供方”走快速设置），最后是折叠的“更多设置”（本地配置导入、图片 / 视频模型）。
import { useEffect, useState } from 'react'
import { useLocation } from 'react-router-dom'
import { ChevronDown } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { pageStateStorage } from '@/lib/page-state-storage'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import { ProviderWorkbench } from './ProviderWorkbench'
import { ProviderConfigModal } from './ProviderDialogs'
import { ProviderDiscovery } from './ProviderDiscovery'
import { providerDiscoveryImportableCount } from './provider-discovery-state'
import { QuickSetupWizard } from './QuickSetupWizard'
import { ModelDefaultsSettings } from './ModelDefaultsSettings'
import { useProviderDiscovery, useProvidersConfig } from './useProvidersConfig'
import { VisualGenerationSettings } from './VisualGenerationSettings'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { ProviderConfig, ProviderType } from './config-types'

import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'

import { FeatureAlert } from '@/components/app/FeaturePage'
import { PageState } from '@/components/app/PageState'

// 连接管理区展开状态持久化：用户显式展开后记住选择，否则保持折叠。
const MANAGE_CONNECTIONS_STORAGE_KEY = 'pisper.config.manageConnectionsOpen'

function storedManageOpen(): boolean | null {
  const stored = pageStateStorage.getItem(MANAGE_CONNECTIONS_STORAGE_KEY)
  return stored === '1' ? true : stored === '0' ? false : null
}

type ModelsSettingsProps = {
  defaultAdvancedExpanded?: boolean
  navigationKey?: number
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
  defaultAdvancedExpanded = false,
  navigationKey,
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
    importRequested || defaultAdvancedExpanded ? true : storedManageOpen(),
  )
  useEffect(() => {
    if (importRequested) setManageOpen(true)
  }, [importRequested])
  useEffect(() => {
    if (defaultAdvancedExpanded) setManageOpen(true)
  }, [defaultAdvancedExpanded, navigationKey])
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
    return settings.error && !settings.loading ? (
      <PageState
        kind="error"
        title={t('config:providerWorkbench.loadFailed')}
        description={settings.error}
        action={
          <Button
            variant="secondary"
            className="max-[650px]:h-11"
            onClick={() => void settings.reload()}
          >
            {t('common:pageTemplate.retry')}
          </Button>
        }
      />
    ) : (
      <PageState
        kind="loading"
        title={t('config:configPage.loadingModelCatalog')}
        description={t('config:configPage.readingProvidersAndAuthenticationStatus')}
      />
    )
  }

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
      <ModelDefaultsSettings
        config={config}
        notify={notify}
        busy={Boolean(settings.settingDefault || settings.toggling)}
        onConfigChanged={settings.applyConfig}
        onSetDefaultModel={settings.setDefaultChatModel}
      />
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
        onAddConnection={() => setWizard({ providerType: 'chat' })}
        meta={
          // 本地配置（Claude Code / Codex）自动接入的读数，跟在分区标题后面。
          <span className="inline-flex items-center gap-1.5" role="status">
            {discovery.discovering && <OrbitSpinner className="size-3" />}
            {discovery.discovering
              ? t('config:providerWorkbench.scanningLocal')
              : t('config:providerWorkbench.localImportStatus', {
                  imported: discovery.autoImport.imported,
                  skipped: discovery.autoImport.skipped,
                })}
          </span>
        }
      />
      {settings.error && <FeatureAlert>{settings.error}</FeatureAlert>}
      {importableCount > 0 && (
        <div className="mt-4 flex flex-wrap items-center gap-x-2 gap-y-0 text-xs text-content-muted">
          <span>{t('config:configPage.localProviderImportHint', { count: importableCount })}</span>
          <Button
            type="button"
            variant="link"
            size="sm"
            className="h-auto min-h-11 shrink-0 px-1 py-0 font-medium sm:min-h-8"
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
            className="group mt-6 mb-3 flex min-h-9 w-full cursor-pointer items-center gap-2 rounded-sm border-0 bg-transparent px-0.5 text-left max-[650px]:min-h-11"
          >
            <ChevronDown
              size={16}
              aria-hidden="true"
              className="shrink-0 text-content-muted transition-transform group-data-[state=closed]:-rotate-90"
            />
            <span className="shrink-0 text-sm font-medium text-content">
              {t('config:providerWorkbench.advanced')}
            </span>
            <span className="min-w-0 truncate text-xs text-content-muted">
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
                // 展开的提供方行在下一帧才渲染，等它出现再滚过去。
                requestAnimationFrame(() =>
                  document
                    .querySelector(`[data-provider-row="${CSS.escape(provider.id)}"]`)
                    ?.scrollIntoView({ block: 'start', behavior: 'smooth' }),
                )
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
