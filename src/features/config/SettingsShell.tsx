import type { ReactNode } from 'react'
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
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'

export type SettingsDestination = { type: 'config'; id: string } | { type: 'page'; id: string }

type SettingsShellProps = {
  activePage: string
  configSection: string
  children: ReactNode
  onNavigate: (destination: SettingsDestination) => void
}

export const CONFIG_SECTIONS = new Set([
  'models',
  'notifications',
  'interface',
  'desktop-pet',
  'updates',
])
export const SETTINGS_PAGES = new Set(['config', 'channels', 'plugins', 'memory', 'mcp', 'skills'])

export function SettingsShell({
  activePage,
  configSection,
  children,
  onNavigate,
}: SettingsShellProps) {
  const { t } = useI18n()
  const groups = [
    {
      label: t('config:settingsShell.agent'),
      items: [
        {
          key: 'config:models',
          label: t('config:configPage.models'),
          icon: Bot,
          destination: { type: 'config', id: 'models' } as const,
        },
      ],
    },
    {
      label: t('config:settingsShell.agentCapabilities'),
      items: [
        {
          key: 'page:plugins',
          label: t('navigation:navigation.tools'),
          icon: Plug,
          destination: { type: 'page', id: 'plugins' } as const,
        },
        {
          key: 'page:mcp',
          label: t('navigation:navigation.mcp'),
          icon: Server,
          destination: { type: 'page', id: 'mcp' } as const,
        },
        {
          key: 'page:skills',
          label: t('navigation:navigation.skills'),
          icon: Sparkles,
          destination: { type: 'page', id: 'skills' } as const,
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
          destination: { type: 'page', id: 'memory' } as const,
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
          destination: { type: 'page', id: 'channels' } as const,
        },
        {
          key: 'config:notifications',
          label: t('config:configPage.notifications'),
          icon: Bell,
          destination: { type: 'config', id: 'notifications' } as const,
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
          destination: { type: 'config', id: 'interface' } as const,
        },
        {
          key: 'config:desktop-pet',
          label: t('config:configPage.desktopPet'),
          icon: Bot,
          destination: { type: 'config', id: 'desktop-pet' } as const,
        },
        {
          key: 'config:updates',
          label: t('config:configPage.appUpdates'),
          icon: RefreshCw,
          destination: { type: 'config', id: 'updates' } as const,
        },
      ],
    },
  ]
  const activeKey = activePage === 'config' ? `config:${configSection}` : `page:${activePage}`

  return (
    <div className="settings-shell">
      <aside className="settings-nav" aria-label={t('config:settingsShell.settingsNavigation')}>
        {groups.map((group) => (
          <section key={group.label}>
            <h2>{group.label}</h2>
            {group.items.map((item) => {
              const Icon = item.icon
              return (
                <button
                  className={activeKey === item.key ? 'active' : ''}
                  aria-current={activeKey === item.key ? 'page' : undefined}
                  onClick={() => onNavigate(item.destination)}
                  key={item.key}
                >
                  <Icon size={15} />
                  <span>{item.label}</span>
                </button>
              )
            })}
          </section>
        ))}
      </aside>
      <div className="settings-content">{children}</div>
    </div>
  )
}
