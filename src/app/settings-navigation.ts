// 设置页导航：把配置分区（models/notifications/...）与独立设置页面
// （插件/MCP/技能/记忆/渠道）组织成分组式侧边栏。destination 区分
// “配置分区”与“独立页面”两种跳转目标，供壳层统一处理高亮。
import {
  Bell,
  Bot,
  Brain,
  Monitor,
  Plug,
  RadioTower,
  RefreshCw,
  Server,
  Sparkles,
  type LucideIcon,
} from 'lucide-react'
import type { Translate } from '@/app/navigation'

export type SettingsDestination = { type: 'config'; id: string } | { type: 'page'; id: string }

export type SettingsNavigationGroup = {
  label: string
  items: Array<{
    key: string
    label: string
    icon: LucideIcon
    destination: SettingsDestination
  }>
}

export const CONFIG_SECTIONS = new Set([
  'models',
  'notifications',
  'interface',
  'desktop-pet',
  'updates',
])

export const SETTINGS_PAGES = new Set(['config', 'channels', 'plugins', 'memory', 'mcp', 'skills'])

// 设置侧边栏分组导航（Agent/能力/上下文/连接/应用）。
export function getSettingsNavigation(t: Translate): SettingsNavigationGroup[] {
  return [
    {
      label: t('config:settingsShell.agent'),
      items: [
        {
          key: 'config:models',
          label: t('config:configPage.models'),
          icon: Bot,
          destination: { type: 'config', id: 'models' },
        },
      ],
    },
    {
      label: t('config:settingsShell.agentCapabilities'),
      items: [
        {
          key: 'page:plugins',
          label: t('navigation:navigation.plugins'),
          icon: Plug,
          destination: { type: 'page', id: 'plugins' },
        },
        {
          key: 'page:mcp',
          label: t('navigation:navigation.mcp'),
          icon: Server,
          destination: { type: 'page', id: 'mcp' },
        },
        {
          key: 'page:skills',
          label: t('navigation:navigation.skills'),
          icon: Sparkles,
          destination: { type: 'page', id: 'skills' },
        },
      ],
    },
    {
      label: t('config:settingsShell.contextAndData'),
      items: [
        {
          key: 'page:memory',
          label: t('navigation:navigation.memory'),
          icon: Brain,
          destination: { type: 'page', id: 'memory' },
        },
      ],
    },
    {
      label: t('config:settingsShell.connections'),
      items: [
        {
          key: 'page:channels',
          label: t('navigation:navigation.channels'),
          icon: RadioTower,
          destination: { type: 'page', id: 'channels' },
        },
        {
          key: 'config:notifications',
          label: t('config:configPage.notifications'),
          icon: Bell,
          destination: { type: 'config', id: 'notifications' },
        },
      ],
    },
    {
      label: t('config:settingsShell.application'),
      items: [
        {
          key: 'config:interface',
          label: t('config:configPage.interface'),
          icon: Monitor,
          destination: { type: 'config', id: 'interface' },
        },
        {
          key: 'config:desktop-pet',
          label: t('config:configPage.desktopPet'),
          icon: Bot,
          destination: { type: 'config', id: 'desktop-pet' },
        },
        {
          key: 'config:updates',
          label: t('config:configPage.appUpdates'),
          icon: RefreshCw,
          destination: { type: 'config', id: 'updates' },
        },
      ],
    },
  ]
}

// 设置导航高亮键：配置分区用 config:section，独立页面用 page:id。
export function settingsNavigationKey(page: string, configSection: string) {
  return page === 'config' ? `config:${configSection}` : `page:${page}`
}
