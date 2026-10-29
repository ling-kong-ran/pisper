// 命令面板的页面分组：一级标签（看板/组件/更多/设置）在前，“更多”里的功能页在后。
// 用数组而非对象保持分组顺序稳定；图标较多，只随命令面板懒加载。
import {
  Blocks,
  Brain,
  CalendarClock,
  FolderOpen,
  History,
  LayoutGrid,
  Plug,
  RadioTower,
  Server,
  Settings,
  Sparkles,
  Workflow,
} from 'lucide-react'
import { LaneBarsIcon } from '@/components/app/LaneBarsIcon'
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
    ['board', t('navigation:shell.board'), LaneBarsIcon],
    ['components', t('navigation:shell.components'), Blocks],
    ['more', t('navigation:shell.more'), LayoutGrid],
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
      t('navigation:shell.more'),
      features.filter(([page]) => runtimePageAvailable(capabilities, page)),
    ],
  ]
}
