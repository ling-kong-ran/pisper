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

export function settingsNavigationKey(page: string, configSection: string) {
  return page === 'config' ? `config:${configSection}` : `page:${page}`
}
