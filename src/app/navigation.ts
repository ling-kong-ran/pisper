import {
  Brain,
  CalendarClock,
  FolderOpen,
  MessageSquare,
  Plug,
  RadioTower,
  Server,
  Settings,
  Sparkles,
  Workflow,
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { PageId } from './routes'

export type Translate = (message: string, values?: Record<string, unknown>) => string
export type Navigation = Array<[string, Array<[PageId, string, LucideIcon]>]>
export type PageMeta = readonly [string, string]

export function getNavigation(t: Translate = (value) => value): Navigation {
  return [
    [
      t('navigation:navigation.workbench'),
      [
        ['chat', t('navigation:navigation.chat'), MessageSquare],
        ['assets', t('navigation:navigation.assets'), FolderOpen],
        ['channels', t('navigation:navigation.channels'), RadioTower],
        ['schedules', t('navigation:navigation.schedules'), CalendarClock],
      ],
    ],
    [
      t('navigation:navigation.capabilities'),
      [
        ['plugins', t('navigation:navigation.tools'), Plug],
        ['memory', t('navigation:navigation.memory'), Brain],
        ['mcp', t('navigation:navigation.mcp'), Server],
        ['skills', t('navigation:navigation.skills'), Sparkles],
        ['workflows', t('navigation:navigation.workflows'), Workflow],
      ],
    ],
    [
      t('navigation:navigation.system'),
      [['config', t('navigation:navigation.settings'), Settings]],
    ],
  ]
}

export function getPageMeta(t: Translate = (value) => value): Record<PageId, PageMeta> {
  return {
    chat: [t('navigation:navigation.chat'), t('navigation:navigation.chatDescription')],
    chatHistory: [
      t('navigation:navigation.chatHistory'),
      t('navigation:navigation.chatHistoryDescription'),
    ],
    assets: [t('navigation:navigation.assets'), t('navigation:navigation.assetsDescription')],
    channels: [t('navigation:navigation.channels'), t('navigation:navigation.channelsDescription')],
    schedules: [
      t('navigation:navigation.schedules'),
      t('navigation:navigation.schedulesDescription'),
    ],
    config: [t('navigation:navigation.settings'), t('navigation:navigation.settingsDescription')],
    plugins: [t('navigation:navigation.tools'), t('navigation:navigation.toolsDescription')],
    memory: [t('navigation:navigation.memory'), t('navigation:navigation.memoryDescription')],
    mcp: [t('navigation:navigation.mcp'), t('navigation:navigation.mcpDescription')],
    skills: [t('navigation:navigation.skills'), t('navigation:navigation.skillsDescription')],
    workflows: [
      t('navigation:navigation.workflows'),
      t('navigation:navigation.workflowsDescription'),
    ],
    workflowCreate: [
      t('navigation:navigation.newWorkflow'),
      t('navigation:navigation.newWorkflowDescription'),
    ],
  }
}
