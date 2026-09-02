// 模型设置页：快速配置向导是唯一配置主路径。
// 结构：当前模型摘要 → 连接管理（本地导入/连接列表/运行策略，默认折叠）
// → 视觉生成专区。折叠状态持久化到 localStorage。
import { useState } from 'react'
import { ChevronDown, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { ConnectionList } from './ConnectionList'
import { CurrentModelSummary } from './CurrentModelSummary'
import { ProviderConfigModal } from './ProviderDialogs'
import { ProviderDiscovery } from './ProviderDiscovery'
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
  const [wizard, setWizard] = useState<WizardTarget | null>(null)
  // 连接弹窗按需新建或编辑；视觉连接也必须能修改 Key、URL 和模型定义。
  const [providerModal, setProviderModal] = useState<{
    providerType: ProviderType
    provider?: ProviderConfig
  } | null>(null)
  const [manageOpen, setManageOpen] = useState<boolean | null>(storedManageOpen)
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
  // 管理区默认折叠（新用户也一样）：首屏只留摘要 + 视觉生成。
  const manageOpenEffective = manageOpen ?? false
  const setManageOpenPersisted = (open: boolean) => {
    setManageOpen(open)
    window.localStorage.setItem(MANAGE_CONNECTIONS_STORAGE_KEY, open ? '1' : '0')
  }
  // 从列表/摘要卡进入向导：复用连接数据，但仍从 Base URL 步骤开始，便于检查端点。
  const openWizardFor = (provider: ProviderConfig) =>
    setWizard({ providerId: provider.id, providerType: provider.type })

  return (
    <>
      <CurrentModelSummary
        config={config}
        onQuickSetup={() => setWizard({ providerType: 'chat' })}
        onChangeModel={() =>
          defaultProvider ? openWizardFor(defaultProvider) : setWizard({ providerType: 'chat' })
        }
      />
      <Collapsible
        open={manageOpenEffective}
        onOpenChange={setManageOpenPersisted}
        data-config-card="models-connections"
      >
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
          <ConnectionList
            providers={config.providers}
            defaultProviderId={defaultProviderId}
            toggling={settings.toggling}
            onConfigure={(provider) => openWizardFor(provider)}
            onToggle={settings.toggleProvider}
            onDelete={settings.deleteProvider}
            onAddCustom={() => setProviderModal({ providerType: 'chat' })}
          />
          {settings.error && <AppError>{settings.error}</AppError>}
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
