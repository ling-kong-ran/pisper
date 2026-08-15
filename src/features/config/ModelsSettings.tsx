import { useState } from 'react'
import { RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { CredentialSettings } from './CredentialSettings'
import { ProviderConnections, ProviderModelCatalog } from './ProviderCatalog'
import { ProviderConfigModal, ProviderModelModal } from './ProviderDialogs'
import { ProviderDiscovery } from './ProviderDiscovery'
import { ProviderSettingsActions, RuntimePolicySettings, RuntimeStatus } from './RuntimeSettings'
import { SettingsCard } from './settings-primitives'
import { useConfigSettings, useProviderDiscovery } from './useConfigSettings'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

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
  const [providerModal, setProviderModal] = useState(false)
  const [modelModal, setModelModal] = useState<'discover' | 'manual' | ''>('')
  const settings = useConfigSettings({ notify, requestConfirm, t })
  const discovery = useProviderDiscovery({
    requestConfirm,
    onImported: settings.applyImportedProvider,
    t,
  })
  usePagePrimaryAction(registerPrimaryAction, () => setProviderModal(true))

  if (!settings.config || !settings.draft || !settings.selectedProvider) {
    return (
      <SettingsCard className="empty-state">
        <RefreshCw className="spin" size={24} />
        <h2>{t('config:configPage.loadingModelCatalog')}</h2>
        <p>{t('config:configPage.readingProvidersAndAuthenticationStatus')}</p>
        {settings.error && <div className="config-error">{settings.error}</div>}
      </SettingsCard>
    )
  }

  const { config, draft, selectedProvider } = settings
  const visualOnly = draft.providerType === 'visual'
  const codexOAuth = selectedProvider.id === 'openai-codex'

  return (
    <>
      <ProviderDiscovery
        discovery={discovery.discovery}
        discovering={discovery.discovering}
        error={discovery.error || discovery.operationError}
        importing={discovery.importing}
        onRefresh={discovery.refresh}
        onImport={discovery.importProvider}
      />
      <div className="split-list-detail config-layout">
        <ProviderConnections
          providers={config.providers}
          selectedProviderId={draft.provider}
          toggling={settings.toggling}
          onAdd={() => setProviderModal(true)}
          onSelect={settings.selectProvider}
          onToggle={settings.toggleProvider}
        />
        <div className="detail-stack">
          <ProviderSettingsActions
            provider={selectedProvider}
            visualOnly={visualOnly}
            codexOAuth={codexOAuth}
            saving={settings.saving}
            dirty={settings.dirty}
            error={settings.error}
            hasModel={Boolean(draft.model)}
            isDefault={
              (config.defaultProvider || config.provider) === selectedProvider.id &&
              (config.defaultModel || config.model) === draft.model
            }
            onSave={settings.save}
          />
          <CredentialSettings
            provider={selectedProvider}
            draft={draft}
            toggling={settings.toggling}
            apiKeyInputRef={settings.apiKeyInputRef}
            onPatchDraft={settings.patchDraft}
            onSelectProviderType={settings.selectProviderType}
            onToggleProvider={settings.toggleProvider}
            onDeleteProvider={settings.deleteProvider}
          />
          <SettingsCard>
            <ProviderModelCatalog
              provider={selectedProvider}
              draft={draft}
              onPatchDraft={settings.patchDraft}
              onSelectModel={settings.selectModel}
              onOpenModelDialog={setModelModal}
            />
          </SettingsCard>
          <div className="config-bottom">
            <RuntimePolicySettings draft={draft} onPatchDraft={settings.patchDraft} />
            <RuntimeStatus provider={selectedProvider} />
          </div>
        </div>
      </div>
      {providerModal && (
        <ProviderConfigModal
          onClose={() => setProviderModal(false)}
          onCreated={(data) => {
            settings.applyCreatedProvider(data)
            setProviderModal(false)
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
