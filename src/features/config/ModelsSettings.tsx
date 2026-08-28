// 模型设置：顶部「当前模型」摘要 + 快速配置向导构成首屏；
// 连接管理（凭据/模型目录/高级端点）默认对已配置用户折叠，需要时展开。
import { useRef, useState } from 'react'
import { ChevronDown, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { CurrentModelSummary } from './CurrentModelSummary'
import { ProviderConnections, ProviderMobilePicker } from './ProviderCatalog'
import { ProviderDetail } from './ProviderDetail'
import { ProviderConfigModal, ProviderModelModal } from './ProviderDialogs'
import { ProviderDiscovery } from './ProviderDiscovery'
import { QuickSetupWizard } from './QuickSetupWizard'
import { RuntimePolicySettings } from './RuntimeSettings'
import { useConfigSettings, useProviderDiscovery } from './useConfigSettings'
import { VisualGenerationSettings } from './VisualGenerationSettings'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { ProviderType } from './config-types'

import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'

import { AppError, AppEmptyState } from '@/components/ui/app-primitives'

// 连接管理区展开状态持久化：用户显式切换后记住选择；
// 未选择过时按「是否已有可用默认模型」决定默认展开/折叠。
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

export function ModelsSettings({
  notify,
  registerPrimaryAction,
  requestConfirm,
}: ModelsSettingsProps) {
  const { t } = useI18n()
  const [wizardOpen, setWizardOpen] = useState(false)
  // 高级「新建连接」弹窗：记录初始用途（从视觉专区打开时预选 visual）。
  const [providerModal, setProviderModal] = useState<ProviderType | ''>('')
  const [modelModal, setModelModal] = useState<'discover' | 'manual' | ''>('')
  const [manageOpen, setManageOpen] = useState<boolean | null>(storedManageOpen)
  const detailRef = useRef<HTMLDivElement>(null)
  const settings = useConfigSettings({ notify, requestConfirm, t })
  const discovery = useProviderDiscovery({
    requestConfirm,
    onImported: settings.applyImportedProvider,
    t,
  })
  // 页面主操作 = 快速配置向导（三步完成对话模型配置）。
  usePagePrimaryAction(registerPrimaryAction, () => setWizardOpen(true))

  if (!settings.config || !settings.draft || !settings.selectedProvider) {
    return (
      <AppEmptyState>
        <RefreshCw className="animate-spin" size={24} />
        <h2>{t('config:configPage.loadingModelCatalog')}</h2>
        <p>{t('config:configPage.readingProvidersAndAuthenticationStatus')}</p>
        {settings.error && <AppError>{settings.error}</AppError>}
      </AppEmptyState>
    )
  }

  const { config, draft, selectedProvider } = settings
  // 已有可用默认模型时管理区默认折叠：首屏只留摘要 + 视觉生成；新用户保持展开。
  const defaultProvider = config.providers.find(
    (item) => item.id === (config.defaultProvider || config.provider),
  )
  const ready = Boolean(
    defaultProvider?.configured &&
    defaultProvider?.enabled &&
    (config.defaultModel || config.model),
  )
  const manageOpenEffective = manageOpen ?? !ready
  const setManageOpenPersisted = (open: boolean) => {
    setManageOpen(open)
    window.localStorage.setItem(MANAGE_CONNECTIONS_STORAGE_KEY, open ? '1' : '0')
  }
  const selectProvider = (provider: (typeof config.providers)[number]) => {
    settings.selectProvider(provider)
    if (!window.matchMedia('(max-width: 900px)').matches) return

    window.setTimeout(() => {
      if (!provider.configured && provider.id !== 'openai-codex') {
        settings.apiKeyInputRef.current?.scrollIntoView({ behavior: 'smooth', block: 'center' })
        settings.apiKeyInputRef.current?.focus({ preventScroll: true })
      } else {
        detailRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' })
      }
    }, 350)
  }

  return (
    <>
      <CurrentModelSummary
        config={config}
        onQuickSetup={() => setWizardOpen(true)}
        onSelectProvider={(providerId) => {
          const provider = config.providers.find((item) => item.id === providerId)
          if (!provider) return
          // 「更改」直达该 Provider 详情：同时展开管理区。
          setManageOpenPersisted(true)
          selectProvider(provider)
        }}
      />
      <Collapsible open={manageOpenEffective} onOpenChange={setManageOpenPersisted}>
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className="group flex w-full cursor-pointer items-center gap-[7px] [margin:2px_0_10px] border-0 bg-transparent p-[4px_2px] text-left"
          >
            <ChevronDown
              size={15}
              className="text-[var(--text-muted)] transition-transform group-data-[state=closed]:-rotate-90"
            />
            <span className="text-[13px] font-[700] text-[var(--text-secondary)]">
              {t('config:configPage.manageConnections')}
            </span>
            <span className="text-[12px] text-[var(--text-tertiary)]">
              {t('config:configPage.manageConnectionsHint')}
            </span>
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <ProviderDiscovery
            discovery={discovery.discovery}
            discovering={discovery.discovering}
            error={discovery.error || discovery.operationError}
            importing={discovery.importing}
            onRefresh={discovery.refresh}
            onImport={discovery.importProvider}
          />
          <div className="split-list-detail config-layout grid min-h-full grid-cols-[300px_minmax(0,1fr)] items-start gap-3 max-[900px]:grid-cols-1">
            <ProviderConnections
              className="max-[900px]:hidden"
              providers={config.providers}
              selectedProviderId={draft.provider}
              toggling={settings.toggling}
              onAdd={() => setWizardOpen(true)}
              onSelect={selectProvider}
              onToggle={settings.toggleProvider}
            />
            <ProviderMobilePicker
              providers={config.providers}
              selectedProviderId={draft.provider}
              onAdd={() => setWizardOpen(true)}
              onSelect={selectProvider}
            />
            <div className="flex min-w-0 scroll-mt-2 flex-col gap-[12px]" ref={detailRef}>
              <ProviderDetail
                provider={selectedProvider}
                draft={draft}
                toggling={settings.toggling}
                saving={settings.saving}
                dirty={settings.dirty}
                error={settings.error}
                isDefault={
                  (config.defaultProvider || config.provider) === selectedProvider.id &&
                  (config.defaultModel || config.model) === draft.model
                }
                apiKeyInputRef={settings.apiKeyInputRef}
                onPatchDraft={settings.patchDraft}
                onSelectProviderType={settings.selectProviderType}
                onSelectModel={settings.selectModel}
                onToggleProvider={settings.toggleProvider}
                onDeleteProvider={settings.deleteProvider}
                onOpenModelDialog={setModelModal}
                onSave={settings.save}
              />
              <RuntimePolicySettings draft={draft} onPatchDraft={settings.patchDraft} />
            </div>
          </div>
        </CollapsibleContent>
      </Collapsible>
      <VisualGenerationSettings
        config={config}
        notify={notify}
        onConfigChanged={settings.applyConfigUpdate}
        onAddVisualProvider={() => setProviderModal('visual')}
      />
      {wizardOpen && (
        <QuickSetupWizard
          config={config}
          onClose={() => setWizardOpen(false)}
          onCompleted={(data, providerId, modelId) => {
            settings.applyWizardCompleted(data, providerId, modelId)
            setWizardOpen(false)
          }}
          onCustomProvider={() => {
            setWizardOpen(false)
            setProviderModal('chat')
          }}
        />
      )}
      {providerModal && (
        <ProviderConfigModal
          initialProviderType={providerModal}
          onClose={() => setProviderModal('')}
          onCreated={(data) => {
            settings.applyCreatedProvider(data)
            setProviderModal('')
          }}
        />
      )}
      {modelModal && (
        <ProviderModelModal
          provider={{ ...selectedProvider, type: draft.providerType }}
          connectionDraft={{
            providerType: draft.providerType,
            api: draft.api || selectedProvider.api,
            baseUrl: draft.baseUrl,
            apiKey: draft.apiKey,
            organization: draft.organization,
          }}
          autoDiscover={modelModal === 'discover'}
          onClose={() => setModelModal('')}
          onSynchronized={settings.applySynchronizedModels}
          onCreated={(data, modelId) => {
            settings.applyCreatedModels(data, selectedProvider.id, modelId)
            setModelModal('')
          }}
        />
      )}
    </>
  )
}
