// 配置页：按分区（模型/通知/界面/快捷键/更新/关于）组织设置，每个分区一个设置组件，共享设置原语。
// 设置主区与功能页同一套模板（规格 2.6、6.4、A6 第 4 批，效果图 desktop-settings-light.png）：52 拖拽区 +
// 页面自己的滚动区 + 居中 760 栏，页标题是短名（模型 / 界面 / 通知 / 快捷键 / 更新 / 关于）+ 一句说明；
// 原共享页头的“设置 / 模型配置”面包屑与第二个大标题和侧栏设置目录重复，已去掉。设置搜索在桌面侧栏里，
// 手机外壳在“‹ 设置”下面（应用壳负责）。模型分区的“快速设置”在页内工具行，仍经应用壳的主操作注册表触发
// （新手引导“去设置”会在模型页挂载前排队）。
// /config 下的远程访问、手机服务器、桌面宠物是功能页，交给 DeviceFeaturePage（功能页模板）。
// 内容根带 data-config-card="section" 锚点，供设置搜索结果跳转高亮定位。
import type { ReactNode } from 'react'
import { Plus } from 'lucide-react'
import type { Translate } from '@/app/navigation'
import {
  isSettingsSection,
  settingsSectionShortLabel,
  type SettingsSection,
} from '@/app/settings-navigation'
import { useI18n } from '@/app/use-i18n'
import { FeaturePage, FeatureToolbar } from '@/components/app/FeaturePage'
import { Button } from '@/components/ui/button'
import { AboutSettings } from './AboutSettings'
import { CONFIG_SECTION_ANCHOR, useConfigCardHighlight } from './config-search'
import { DeviceFeaturePage } from './DeviceFeaturePage'
import { InterfaceSettings } from './InterfaceSettings'
import { ShortcutSettings } from './ShortcutSettings'
import { ModelsSettings } from './ModelsSettings'
import { NotificationSettings } from './NotificationSettings'
import { UpdateSettings } from './UpdateSettings'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { NotificationSettingsData } from '@/types/notifications'
import type { AppUpdateController } from '@/types/update'

type ConfigPageProps = {
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  // 触发已注册的主操作（模型分区的“快速设置”）。
  invokePrimaryAction: () => void
  section: string
  // 远程访问 / 手机服务器 / 桌面宠物是功能页：从功能进入时由应用壳给出“‹ 功能”（规格 2.4）。
  onBackToFeatures?: () => void
  onBrowserNotificationChange?: (settings: NotificationSettingsData) => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  update: AppUpdateController
  renderInterfaceSettings?: (appearance: ReactNode) => ReactNode
}

// 页标题下的一句说明（键必须是字面量，i18n 检查逐个核对中英两份）。
function settingsLede(t: Translate, section: SettingsSection) {
  switch (section) {
    case 'interface':
      return t('config:settingsShell.lede.interface')
    case 'notifications':
      return t('config:settingsShell.lede.notifications')
    case 'shortcuts':
      return t('config:settingsShell.lede.shortcuts')
    case 'updates':
      return t('config:settingsShell.lede.updates')
    case 'about':
      return t('config:settingsShell.lede.about')
    default:
      return t('config:settingsShell.lede.models')
  }
}

export function ConfigPage({
  notify,
  registerPrimaryAction,
  invokePrimaryAction,
  section,
  onBackToFeatures,
  onBrowserNotificationChange,
  requestConfirm,
  update,
  renderInterfaceSettings,
}: ConfigPageProps) {
  const { t } = useI18n()
  // 分区切换后消费搜索跳转的高亮请求（同分区点击由事件即时触发）。
  const searchTarget = useConfigCardHighlight(section)
  // 功能 › 设备与连接的三个分区不是设置：用功能页模板，不套设置外壳（规格 2.4、A6 第 3 批）。
  if (section === 'remote-access' || section === 'mobile-server' || section === 'desktop-pet')
    return (
      <DeviceFeaturePage
        section={section}
        notify={notify}
        requestConfirm={requestConfirm}
        onBack={onBackToFeatures}
      />
    )
  // 未知分区与路由一样回退到模型。
  const current: SettingsSection = isSettingsSection(section) ? section : 'models'
  let content
  if (current === 'notifications') {
    content = (
      <NotificationSettings
        notify={notify}
        onBrowserNotificationChange={onBrowserNotificationChange}
      />
    )
  } else if (current === 'interface') {
    const appearance = (
      <InterfaceSettings
        notify={notify}
        defaultAdvancedExpanded={searchTarget?.expandGroup === 'interface-advanced'}
        navigationKey={searchTarget?.navigationKey}
      />
    )
    content = renderInterfaceSettings ? renderInterfaceSettings(appearance) : appearance
  } else if (current === 'shortcuts') {
    content = <ShortcutSettings requestConfirm={requestConfirm} />
  } else if (current === 'updates') {
    content = <UpdateSettings notify={notify} update={update} />
  } else if (current === 'about') {
    content = <AboutSettings update={update} notify={notify} />
  } else {
    content = (
      <ModelsSettings
        defaultAdvancedExpanded={searchTarget?.expandGroup === 'models-advanced'}
        navigationKey={searchTarget?.navigationKey}
        notify={notify}
        registerPrimaryAction={registerPrimaryAction}
        requestConfirm={requestConfirm}
      />
    )
  }

  const quickSetup = t('navigation:pageHeader.quickSetup')
  return (
    <FeaturePage
      data-settings-page={current}
      title={settingsSectionShortLabel(t, current)}
      description={settingsLede(t, current)}
      toolbar={
        current === 'models' ? (
          <FeatureToolbar
            actions={
              <Button variant="outline" data-settings-quick-setup onClick={invokePrimaryAction}>
                <Plus className="size-4" aria-hidden="true" />
                {quickSetup}
              </Button>
            }
          />
        ) : undefined
      }
    >
      <div data-config-card={CONFIG_SECTION_ANCHOR} className="mt-6 min-w-0 max-[650px]:mt-3">
        {content}
      </div>
    </FeaturePage>
  )
}
