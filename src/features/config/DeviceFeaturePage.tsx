// 功能 › 设备与连接的三个页面（/config/remote-access、/config/mobile-server、/config/desktop-pet，规格 2.4、2.5、2.7）：
// 路径仍在 /config 下（旧链接、通知、设置搜索照常），但它们是功能页——用功能页模板（“‹ 功能”、h1、一句说明、分区与列表行），
// 不用设置外壳。平台判断集中在 deviceFeatureSupport（与功能索引的卡片同一份结论）：不支持的端显示一行禁用的开关并写明原因，
// 不去调用接口。
import { Bot, MonitorSmartphone } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { FeatureList, FeaturePage, FeatureRow, FeatureSection } from '@/components/app/FeaturePage'
import { Switch } from '@/components/ui/switch'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import {
  deviceFeatureSupport,
  type DeviceFeatureSection,
  type DeviceFeatureSupport,
} from '@/types/runtime-capabilities'
import { CONFIG_SECTION_ANCHOR } from './config-search'
import { DesktopPetSettings } from './DesktopPetSettings'
import { MobileServerSettings } from './MobileServerSettings'
import { RemoteAccessSettings } from './RemoteAccessSettings'
import { RemoteWorkspaceSettings } from './RemoteWorkspaceSettings'

export function DeviceFeaturePage({
  section,
  notify,
  requestConfirm,
  onBack,
}: {
  section: DeviceFeatureSection
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  onBack?: () => void
}) {
  const { t } = useI18n()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const mobileApp = useIsMobileApp()
  const support = deviceFeatureSupport(capabilities, section, { mobileApp })
  const title =
    section === 'remote-access'
      ? t('config:configPage.remoteAccess')
      : section === 'mobile-server'
        ? t('config:configPage.mobileServer')
        : t('config:configPage.desktopPet')
  const description =
    section === 'remote-access'
      ? t('config:deviceFeature.remoteAccessLede')
      : section === 'mobile-server'
        ? t('config:deviceFeature.mobileServerLede')
        : t('config:desktopPetSettings.description')

  let content
  if (!support.supported) content = <UnsupportedDevice section={section} support={support} />
  else if (section === 'desktop-pet')
    content = <DesktopPetSettings notify={notify} requestConfirm={requestConfirm} />
  else if (section === 'mobile-server')
    content = <MobileServerSettings requestConfirm={requestConfirm} />
  else
    content = (
      <>
        {!window.__PISPER_REMOTE_WORKSPACE__ && <RemoteAccessSettings notify={notify} />}
        <RemoteWorkspaceSettings requestConfirm={requestConfirm} />
      </>
    )

  return (
    <FeaturePage
      data-device-feature={section}
      title={title}
      description={description}
      back={
        onBack && {
          label: t('navigation:sidebar.features'),
          ariaLabel: t('navigation:shell.backToFeaturesLabel'),
          onBack,
        }
      }
    >
      {/* 设置搜索跳到这三个分区时按这个锚点定位（与设置分区同一机制）。 */}
      <div data-config-card={CONFIG_SECTION_ANCHOR} className="min-w-0">
        {content}
      </div>
    </FeaturePage>
  )
}

// 不支持的端：一行禁用的开关 + 原因（规格 2.5“不支持的端显示为禁用并写明原因”），不只靠变灰。
function UnsupportedDevice({
  section,
  support,
}: {
  section: DeviceFeatureSection
  support: Extract<DeviceFeatureSupport, { supported: false }>
}) {
  const { t } = useI18n()
  const name =
    section === 'desktop-pet'
      ? t('config:desktopPetSettings.showOnDesktop')
      : section === 'mobile-server'
        ? t('config:mobileServer.title')
        : t('config:remoteAccess.allowConnections')
  const reason =
    support.reason === 'runtime'
      ? t('config:deviceFeature.runtimeUnavailable')
      : support.reason === 'mobile-only'
        ? t('config:deviceFeature.mobileOnly')
        : section === 'desktop-pet'
          ? t('config:desktopPetSettings.desktopOnlyDescription')
          : t('config:deviceFeature.remoteAccessDesktopOnly')
  return (
    <FeatureSection title={t('config:deviceFeature.unavailableTitle')} data-device-unsupported>
      <FeatureList>
        <FeatureRow
          icon={section === 'desktop-pet' ? <Bot /> : <MonitorSmartphone />}
          name={name}
          description={reason}
          status={t('config:deviceFeature.unavailable')}
          actions={<Switch checked={false} disabled aria-label={name} />}
        />
      </FeatureList>
    </FeatureSection>
  )
}
