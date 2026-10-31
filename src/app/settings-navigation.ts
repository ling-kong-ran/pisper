// 设置目录（规格 v2 第 2.6 节）：设置只放应用本身的设置——模型 · 界面 · 通知 · 快捷键 · 更新 · 关于。
// 插件、MCP、技能、星忆、渠道、远程访问 / 手机服务器、桌面宠物只在“功能”里（只有一个家）：
// 旧的双归属机制（SETTINGS_PAGES 里的功能页、从功能栏进入时写进历史条目的 ACTIVITY_NAVIGATION_STATE）已退役。
// /config/remote-access、/config/mobile-server、/config/desktop-pet 路径不变（旧链接、通知、守卫照常），
// 但它们作为功能页打开：点亮“功能”、页头“‹ 功能”、不用设置外壳。
// 纯数据与判断，不依赖 React；桌面设置侧栏、手机设置分区条与页头共用同一份结论。
import { Bell, Bot, Info, Keyboard, Monitor, RefreshCw, type LucideIcon } from 'lucide-react'
import type { Translate } from '@/app/navigation'
import {
  LEGACY_RUNTIME_CAPABILITIES,
  runtimeConfigSectionAvailable,
  type RuntimeCapabilities,
} from '@/types/runtime-capabilities'

// 设置外壳里的分区，按目录顺序。
export const APP_SETTINGS_SECTIONS = Object.freeze([
  'models',
  'interface',
  'notifications',
  'shortcuts',
  'updates',
  'about',
] as const)

// 挂在 /config/:section 下、但属于“功能 › 设备与连接”的分区（规格 2.4、2.7）。
export const FEATURE_CONFIG_SECTIONS: ReadonlySet<string> = new Set([
  'remote-access',
  'mobile-server',
  'desktop-pet',
])

// /config/:section 认得的全部分区：未知分区回退到模型。
export const CONFIG_SECTIONS: ReadonlySet<string> = new Set([
  ...APP_SETTINGS_SECTIONS,
  ...FEATURE_CONFIG_SECTIONS,
])

export type SettingsSection = (typeof APP_SETTINGS_SECTIONS)[number]

export type SettingsNavigationItem = {
  section: SettingsSection
  label: string
  icon: LucideIcon
}

// 当前路由是否用设置外壳（侧栏换成设置目录）：只有 /config 下的应用设置分区。
export function settingsOwnsRoute(page: string, configSection: string) {
  return page === 'config' && !FEATURE_CONFIG_SECTIONS.has(configSection)
}

const SECTION_ICONS: Readonly<Record<SettingsSection, LucideIcon>> = {
  models: Bot,
  interface: Monitor,
  notifications: Bell,
  shortcuts: Keyboard,
  updates: RefreshCw,
  about: Info,
}

// /config/:section 的显示名（设置分区与三个功能页都用）。
export function configSectionLabel(t: Translate, section: string) {
  switch (section) {
    case 'interface':
      return t('config:configPage.interface')
    case 'notifications':
      return t('config:configPage.notifications')
    case 'shortcuts':
      return t('config:configPage.shortcuts')
    case 'updates':
      return t('config:configPage.appUpdates')
    case 'about':
      return t('config:configPage.about')
    case 'remote-access':
      return t('config:configPage.remoteAccess')
    case 'mobile-server':
      return t('config:configPage.mobileServer')
    case 'desktop-pet':
      return t('config:configPage.desktopPet')
    default:
      return t('config:configPage.models')
  }
}

// 设置目录：按 Runtime 能力裁掉不可用的分区。桌面设置侧栏与手机设置分区条共用。
export function getSettingsNavigation(
  t: Translate,
  { capabilities = LEGACY_RUNTIME_CAPABILITIES }: { capabilities?: RuntimeCapabilities } = {},
): SettingsNavigationItem[] {
  return APP_SETTINGS_SECTIONS.filter((section) =>
    runtimeConfigSectionAvailable(capabilities, section),
  ).map((section) => ({
    section,
    label: configSectionLabel(t, section),
    icon: SECTION_ICONS[section],
  }))
}
