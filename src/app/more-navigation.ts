// “功能”索引（原“更多”，路径 /more 不变）的分组与卡片（桌面与手机共用同一套数据，规格 2.7）。
// 取代旧功能栏分组 activity-navigation 在外壳里的角色：Runtime 裁掉的页面直接隐藏，
// 只在部分平台提供的能力显示为禁用并写明原因。纯数据、翻译由调用方注入；不依赖 Feature，守卫测试可以直接读取。
import {
  AppWindow,
  Blocks,
  Bot,
  Brain,
  CalendarClock,
  History,
  Image,
  MonitorSmartphone,
  Plug,
  Plus,
  RadioTower,
  Server,
  Sparkles,
  Workflow,
  type LucideIcon,
} from 'lucide-react'
import type { Translate } from '@/app/navigation'
import type { PageId } from '@/app/routes'
import {
  LEGACY_RUNTIME_CAPABILITIES,
  runtimeConfigSectionAvailable,
  runtimePageAvailable,
  type RuntimeCapabilities,
} from '@/types/runtime-capabilities'

export type MoreDestination =
  { type: 'page'; id: PageId } | { type: 'config'; id: string } | { type: 'component'; id: string }

export type MoreTile = {
  key: string
  label: string
  description: string
  icon: LucideIcon
  destination: MoreDestination
  // 平台不支持时显示为禁用卡片，原因写在说明位置，不能只靠变灰表达。
  disabledReason?: string
}

export type MoreGroupId = 'sessions' | 'automation' | 'extensions' | 'devices' | 'components'

export type MoreGroup = {
  id: MoreGroupId
  label: string
  tiles: MoreTile[]
}

export type MoreComponentEntry = { id: string; name: string; description?: string }

// “我的组件”末行“+ 放置组件”的键：它不是一个功能，是空位，画成虚线。
export const ADD_COMPONENT_KEY = 'add:component'

export { MORE_PAGES, MORE_SEARCH_INPUT_ID } from '@/app/more-pages'

type MoreNavigationOptions = {
  capabilities?: RuntimeCapabilities
  // 手机 App 经代理访问：远程访问换成“手机服务器”，桌面宠物显示为禁用。
  mobileApp?: boolean
  // 以整页打开的已安装组件（/tools/components/:id）；缺省时“我的组件”只有“+ 放置组件”。
  components?: readonly MoreComponentEntry[]
}

export function getMoreNavigation(
  t: Translate,
  {
    capabilities = LEGACY_RUNTIME_CAPABILITIES,
    mobileApp = false,
    components = [],
  }: MoreNavigationOptions = {},
): MoreGroup[] {
  const page = (
    id: PageId,
    label: string,
    description: string,
    icon: LucideIcon,
  ): MoreTile | null =>
    runtimePageAvailable(capabilities, id)
      ? { key: `page:${id}`, label, description, icon, destination: { type: 'page', id } }
      : null
  const devices: Array<MoreTile | null> = [
    mobileApp
      ? {
          key: 'config:mobile-server',
          label: t('config:configPage.mobileServer'),
          description: t('navigation:more.mobileServerHint'),
          icon: MonitorSmartphone,
          destination: { type: 'config', id: 'mobile-server' },
        }
      : runtimeConfigSectionAvailable(capabilities, 'remote-access')
        ? {
            key: 'config:remote-access',
            label: t('config:configPage.remoteAccess'),
            description: t('navigation:more.remoteAccessHint'),
            icon: MonitorSmartphone,
            destination: { type: 'config', id: 'remote-access' },
          }
        : null,
    // 桌面宠物只在桌面端可用：手机端显示禁用并写明原因；桌面 Runtime 裁掉时才隐藏。
    mobileApp || runtimeConfigSectionAvailable(capabilities, 'desktop-pet')
      ? {
          key: 'config:desktop-pet',
          label: t('config:configPage.desktopPet'),
          description: t('navigation:more.desktopPetHint'),
          icon: Bot,
          destination: { type: 'config', id: 'desktop-pet' },
          ...(mobileApp ? { disabledReason: t('navigation:more.desktopOnly') } : {}),
        }
      : null,
  ]
  const groups: Array<{ id: MoreGroupId; label: string; tiles: Array<MoreTile | null> }> = [
    {
      id: 'sessions',
      label: t('navigation:more.groupSessions'),
      tiles: [
        page(
          'chatHistory',
          t('navigation:navigation.chatHistory'),
          t('navigation:more.chatHistoryHint'),
          History,
        ),
        page('assets', t('navigation:navigation.assets'), t('navigation:more.assetsHint'), Image),
        page('memory', t('navigation:navigation.memory'), t('navigation:more.memoryHint'), Brain),
      ],
    },
    {
      id: 'automation',
      label: t('navigation:more.groupAutomation'),
      tiles: [
        page(
          'workflows',
          t('navigation:navigation.workflows'),
          t('navigation:more.workflowsHint'),
          Workflow,
        ),
        page(
          'schedules',
          t('navigation:navigation.schedules'),
          t('navigation:more.schedulesHint'),
          CalendarClock,
        ),
        page(
          'channels',
          t('navigation:navigation.channels'),
          t('navigation:more.channelsHint'),
          RadioTower,
        ),
      ],
    },
    {
      id: 'extensions',
      label: t('navigation:more.groupExtensions'),
      tiles: [
        page('plugins', t('navigation:navigation.plugins'), t('navigation:more.pluginsHint'), Plug),
        page(
          'skills',
          t('navigation:navigation.skills'),
          t('navigation:more.skillsHint'),
          Sparkles,
        ),
        page('mcp', t('navigation:navigation.mcp'), t('navigation:more.mcpHint'), Server),
        // 组件工作室不再是一级入口（规格 2.4 / 2.7）：桌面与手机都从“功能 › 扩展”进入。
        {
          key: 'page:components',
          label: t('navigation:shell.components'),
          description: t('navigation:more.componentStudioHint'),
          icon: Blocks,
          destination: { type: 'page', id: 'components' },
        },
      ],
    },
    { id: 'devices', label: t('navigation:more.groupDevices'), tiles: devices },
    {
      id: 'components',
      label: t('navigation:more.groupComponents'),
      tiles: [
        ...components.map((component): MoreTile => ({
          key: `component:${component.id}`,
          label: component.name,
          description: component.description || t('navigation:more.componentHint'),
          icon: AppWindow,
          destination: { type: 'component', id: component.id },
        })),
        // 最后一行固定是“+ 放置组件”，去组件工作室创建或导入；没有组件时整组只有它（规格 2.7）。
        {
          key: ADD_COMPONENT_KEY,
          label: t('navigation:more.addComponent'),
          description: t('navigation:more.addComponentHint'),
          icon: Plus,
          destination: { type: 'page', id: 'components' },
        },
      ],
    },
  ]
  return groups
    .map((group) => ({
      id: group.id,
      label: group.label,
      tiles: group.tiles.filter((tile): tile is MoreTile => tile !== null),
    }))
    .filter((group) => group.tiles.length > 0)
}

// 搜索筛选：按名称、描述与所属分组做不区分大小写的包含匹配；空关键词返回原分组。
export function filterMoreNavigation(groups: readonly MoreGroup[], query: string, locale?: string) {
  const needle = query.trim().toLocaleLowerCase(locale)
  if (!needle) return [...groups]
  return groups
    .map((group) => ({
      ...group,
      tiles: group.tiles.filter((tile) =>
        [tile.label, tile.description, group.label].some((value) =>
          value.toLocaleLowerCase(locale).includes(needle),
        ),
      ),
    }))
    .filter((group) => group.tiles.length > 0)
}
