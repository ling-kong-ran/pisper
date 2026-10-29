// 页面元信息（标题/描述）的集中定义，翻译由调用方注入；getPageMeta 供页头与文档标题使用。
// 命令面板的页面分组（带图标）在 palette-navigation.ts，随命令面板按需加载，不进入启动包。
import type { LucideIcon } from 'lucide-react'
import type { PageId } from './routes'

export type Translate = (message: string, values?: Record<string, unknown>) => string
export type Navigation = Array<[string, Array<[PageId, string, LucideIcon]>]>
export type PageMeta = readonly [string, string]

// 页面元信息（标题/描述）表：供页头与文档标题使用，全部翻译化。
export function getPageMeta(t: Translate = (value) => value): Record<PageId, PageMeta> {
  return {
    board: [t('navigation:shell.board'), t('navigation:shell.boardDescription')],
    session: [t('navigation:navigation.chat'), ''],
    more: [t('navigation:shell.more'), t('navigation:shell.moreDescription')],
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
    components: [
      t('navigation:shell.components'),
      t('navigation:navigation.componentsDescription'),
    ],
    workflows: [
      t('navigation:navigation.workflows'),
      t('navigation:navigation.workflowsDescription'),
    ],
    customComponentTool: [
      t('navigation:navigation.components'),
      t('navigation:navigation.componentsDescription'),
    ],
    workflowCreate: getWorkflowEditorMeta(t, false),
  }
}

// 工作流编辑器的新建与编辑共用一个页面 id：打开已有工作流时页头不能仍写“新建”。
export function getWorkflowEditorMeta(t: Translate, editing: boolean): PageMeta {
  return editing
    ? [t('navigation:navigation.editWorkflow'), t('navigation:navigation.editWorkflowDescription')]
    : [t('navigation:navigation.newWorkflow'), t('navigation:navigation.newWorkflowDescription')]
}
