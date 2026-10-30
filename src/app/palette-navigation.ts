// 命令面板的页面分组：一级入口（会话/全部代理/功能/设置，以及组件工作室）在前，“功能”里的功能页在后。
// 名称与侧栏入口一致（规格 v2 第 2.4 节：看板改名全部代理、更多改名功能，路径与页面 id 不变）。
// “会话”用兼容页面 id 'chat'：应用壳的 navigate('chat') 没有待打开的会话时落到主页（上次停留的会话或
// 新任务草稿），不会像 'session'（/board/new）那样一进入就建会话。
// 用数组而非对象保持分组顺序稳定；图标较多，只随命令面板懒加载。
import {
  Blocks,
  Brain,
  CalendarClock,
  FolderOpen,
  History,
  LayoutGrid,
  List,
  MessageSquare,
  Plug,
  RadioTower,
  Server,
  Settings,
  Sparkles,
  Workflow,
} from 'lucide-react'
import type { Navigation, Translate } from '@/app/navigation'
import {
  LEGACY_RUNTIME_CAPABILITIES,
  runtimePageAvailable,
  type RuntimeCapabilities,
} from '@/types/runtime-capabilities'

export function getNavigation(
  t: Translate = (value) => value,
  capabilities: RuntimeCapabilities = LEGACY_RUNTIME_CAPABILITIES,
): Navigation {
  const shell: Navigation[number][1] = [
    ['chat', t('navigation:navigation.chat'), MessageSquare],
    ['board', t('navigation:sidebar.allAgents'), List],
    ['more', t('navigation:sidebar.features'), LayoutGrid],
    ['components', t('navigation:shell.components'), Blocks],
    ['config', t('navigation:navigation.settings'), Settings],
  ]
  const features: Navigation[number][1] = [
    ['chatHistory', t('navigation:navigation.chatHistory'), History],
    ['assets', t('navigation:navigation.assets'), FolderOpen],
    ['memory', t('navigation:navigation.memory'), Brain],
    ['workflows', t('navigation:navigation.workflows'), Workflow],
    ['schedules', t('navigation:navigation.schedules'), CalendarClock],
    ['channels', t('navigation:navigation.channels'), RadioTower],
    ['plugins', t('navigation:navigation.plugins'), Plug],
    ['skills', t('navigation:navigation.skills'), Sparkles],
    ['mcp', t('navigation:navigation.mcp'), Server],
  ]
  return [
    [t('navigation:navigation.app'), shell],
    [
      t('navigation:sidebar.features'),
      features.filter(([page]) => runtimePageAvailable(capabilities, page)),
    ],
  ]
}
