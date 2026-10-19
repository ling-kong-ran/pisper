import {
  CalendarClock,
  FolderOpen,
  MessageSquare,
  Workflow,
  Blocks,
  History,
  Plug,
  Server,
  Sparkles,
  Brain,
  RadioTower,
} from 'lucide-react'
import type { ActivityGroup } from '@/components/layout/ActivityRail'
import type { Navigation, Translate } from '@/app/navigation'
import {
  LEGACY_RUNTIME_CAPABILITIES,
  runtimePageAvailable,
  type RuntimeCapabilities,
} from '@/types/runtime-capabilities'

export function getActivityNavigation(
  t: Translate = (value) => value,
  capabilities: RuntimeCapabilities = LEGACY_RUNTIME_CAPABILITIES,
): ActivityGroup[] {
  const groups: Array<[string, string, typeof FolderOpen, Navigation[number][1]]> = [
    [
      'workspace',
      t('navigation:rail.workspace'),
      MessageSquare,
      [
        ['chat', t('navigation:navigation.chat'), MessageSquare],
        ['chatHistory', t('navigation:navigation.chatHistory'), History],
      ],
    ],
    [
      'resources',
      t('navigation:rail.resources'),
      FolderOpen,
      [
        ['assets', t('navigation:navigation.assets'), FolderOpen],
        ['memory', t('navigation:navigation.memory'), Brain],
      ],
    ],
    [
      'automation',
      t('navigation:rail.automation'),
      Workflow,
      [
        ['workflows', t('navigation:navigation.workflows'), Workflow],
        ['schedules', t('navigation:navigation.schedules'), CalendarClock],
        ['channels', t('navigation:navigation.channels'), RadioTower],
      ],
    ],
    [
      'extensions',
      t('navigation:rail.extensions'),
      Blocks,
      [
        ['plugins', t('navigation:navigation.plugins'), Plug],
        ['skills', t('navigation:navigation.skills'), Sparkles],
        ['mcp', t('navigation:navigation.mcp'), Server],
        ['components', t('navigation:navigation.components'), Blocks],
      ],
    ],
  ]
  return groups
    .map(([id, label, icon, entries]) => ({
      id,
      label,
      icon,
      items: entries
        .filter(([page]) => runtimePageAvailable(capabilities, page))
        .map(([id, label, icon]) => ({ id, label, icon })),
    }))
    .filter((group) => group.items.length > 0)
}
