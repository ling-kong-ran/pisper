// 侧边栏导航与页面元信息（标题/描述）的集中定义，翻译由调用方注入。
// 用数组而非对象保持分组顺序稳定；getPageMeta 供文档标题与路由描述使用。
import { CalendarClock, FolderOpen, MessageSquare, Workflow } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { PageId } from './routes'

export type Translate = (message: string, values?: Record<string, unknown>) => string
export type Navigation = Array<[string, Array<[PageId, string, LucideIcon]>]>
export type PageMeta = readonly [string, string]

// 工作台导航分组：目前只有“工作台”一组（聊天/资源/工作流/计划），
// 数组保持顺序稳定，图标由调用方渲染。
export function getNavigation(t: Translate = (value) => value): Navigation {
  return [
    [
      t('navigation:navigation.workbench'),
      [
        ['chat', t('navigation:navigation.chat'), MessageSquare],
        ['assets', t('navigation:navigation.assets'), FolderOpen],
        ['workflows', t('navigation:navigation.workflows'), Workflow],
        ['schedules', t('navigation:navigation.schedules'), CalendarClock],
      ],
    ],
  ]
}

// 页面元信息（标题/描述）表：供页头与文档标题使用，全部翻译化。
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
    plugins: [t('navigation:navigation.plugins'), t('navigation:navigation.pluginsDescription')],
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
