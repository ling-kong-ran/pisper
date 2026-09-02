// 设置搜索索引：覆盖各设置分区（分区级条目）与分区内设置卡片（卡片级条目）。
// 每个条目的标题同时收录 zh-CN 与 en-US 两套文案，实现跨语言别名匹配
// （输入 "model" 命中「模型配置」、输入「通知」命中 Notifications）；
// 另用硬编码别名表补充常见口语词（provider/主题/宠物/更新/远程 等）。
import { useEffect } from 'react'
import { translateText, type SupportedLanguage } from '@/app/i18n'

export type ConfigSearchEntry = {
  // 条目唯一 id（别名表指向与去重排序的依据）
  id: string
  // 所属设置分区（config 路由分区 id）
  section: string
  // 点击后要定位高亮的卡片锚点（对应 data-config-card 属性值）；
  // 'section' 表示分区级条目，锚到分区根容器
  card: string
  // 卡片标题的 i18n key（渲染时按当前语言取文案）
  titleKey: string
  // 硬编码双语别名：只参与匹配，不参与展示
  keywords?: readonly string[]
}

export type ConfigSearchMatch = {
  entry: ConfigSearchEntry
  // 当前语言下的卡片标题（下拉列表主文案）
  title: string
  // 当前语言下的分区名（下拉列表次级面包屑）
  sectionTitle: string
}

// 分区根容器锚点：ConfigPage 用 data-config-card="section" 标记内容根。
export const CONFIG_SECTION_ANCHOR = 'section'

// 分区标题 key：供卡片级结果展示所属分区名。
const SECTION_TITLE_KEYS: Record<string, string> = {
  models: 'config:configPage.models',
  notifications: 'config:configPage.notifications',
  interface: 'config:configPage.interface',
  'desktop-pet': 'config:configPage.desktopPet',
  updates: 'config:configPage.appUpdates',
  about: 'config:configPage.about',
  'remote-access': 'config:configPage.remoteAccess',
  'mobile-server': 'config:configPage.mobileServer',
}

const ENTRIES: readonly ConfigSearchEntry[] = [
  // —— 分区级条目：命中后跳到该分区顶部 ——
  {
    id: 'sec-models',
    section: 'models',
    card: CONFIG_SECTION_ANCHOR,
    titleKey: 'config:configPage.models',
    keywords: ['model', 'provider', 'llm', 'api', '模型', '供应商', '大模型'],
  },
  {
    id: 'sec-notifications',
    section: 'notifications',
    card: CONFIG_SECTION_ANCHOR,
    titleKey: 'config:configPage.notifications',
    keywords: ['notification', 'push', 'alert', '通知', '推送', '提醒'],
  },
  {
    id: 'sec-interface',
    section: 'interface',
    card: CONFIG_SECTION_ANCHOR,
    titleKey: 'config:configPage.interface',
    keywords: ['interface', 'theme', 'appearance', 'display', '界面', '主题', '外观', '显示'],
  },
  {
    id: 'sec-desktop-pet',
    section: 'desktop-pet',
    card: CONFIG_SECTION_ANCHOR,
    titleKey: 'config:configPage.desktopPet',
    keywords: ['pet', 'petdex', '宠物', '桌宠'],
  },
  {
    id: 'sec-updates',
    section: 'updates',
    card: CONFIG_SECTION_ANCHOR,
    titleKey: 'config:configPage.appUpdates',
    keywords: ['update', 'upgrade', 'version', '更新', '升级', '版本'],
  },
  {
    id: 'sec-about',
    section: 'about',
    card: CONFIG_SECTION_ANCHOR,
    titleKey: 'config:configPage.about',
    keywords: ['about', 'info', 'license', '关于', '信息', '许可'],
  },
  {
    id: 'sec-remote-access',
    section: 'remote-access',
    card: CONFIG_SECTION_ANCHOR,
    titleKey: 'config:configPage.remoteAccess',
    keywords: ['remote', 'network', 'phone', '远程', '网络', '手机'],
  },
  {
    id: 'sec-mobile-server',
    section: 'mobile-server',
    card: CONFIG_SECTION_ANCHOR,
    titleKey: 'config:configPage.mobileServer',
    keywords: ['server', 'remote', 'network', '服务器', '远程', '网络'],
  },
  // —— models 分区卡片 ——
  {
    id: 'models-current-model',
    section: 'models',
    card: 'models-current-model',
    titleKey: 'config:configPage.currentChatModel',
    keywords: ['model', 'provider', 'quick setup', '模型', '供应商', '快速配置', '对话模型'],
  },
  {
    id: 'models-connections',
    section: 'models',
    card: 'models-connections',
    titleKey: 'config:configPage.manageConnections',
    keywords: ['provider', 'connection', 'api key', 'import', '连接', '供应商', '导入', '密钥'],
  },
  {
    id: 'models-runtime-policy',
    section: 'models',
    card: 'models-connections',
    titleKey: 'config:configPage.agentRuntimePolicy',
    keywords: ['runtime', 'agent', 'policy', '运行策略', '运行时', '执行策略'],
  },
  {
    id: 'models-visual',
    section: 'models',
    card: 'models-visual',
    titleKey: 'config:configPage.currentVisualModel',
    keywords: ['image', 'video', 'visual', '图片', '图像', '视频', '视觉', '生图'],
  },
  // —— notifications 分区卡片 ——
  {
    id: 'notifications-browser',
    section: 'notifications',
    card: 'notifications-browser',
    titleKey: 'config:notificationSettings.notification',
    keywords: ['browser', 'system', 'push', '浏览器通知', '系统通知', '推送'],
  },
  {
    id: 'notifications-templates',
    section: 'notifications',
    card: 'notifications-templates',
    titleKey: 'config:notificationSettings.notificationTemplates',
    keywords: ['template', '模板', '通知模板'],
  },
  // —— interface 分区卡片 ——
  {
    id: 'interface-appearance',
    section: 'interface',
    card: 'interface-appearance',
    titleKey: 'config:interfaceSettings.appearance',
    keywords: [
      'theme',
      'dark',
      'light',
      'accent',
      'font',
      'corner',
      '主题',
      '深色',
      '浅色',
      '强调色',
      '字体',
      '圆角',
      '外观',
    ],
  },
  {
    id: 'interface-motion',
    section: 'interface',
    card: 'interface-motion',
    titleKey: 'config:interfaceSettings.displayAndMotion',
    keywords: ['motion', 'density', 'animation', '动效', '动画', '密度', '显示'],
  },
  {
    id: 'interface-language',
    section: 'interface',
    card: 'interface-language',
    titleKey: 'config:languageSettings.displayLanguage',
    keywords: ['language', 'chinese', 'english', '语言', '中文', '英文', '英语'],
  },
  // —— desktop-pet 分区卡片 ——
  {
    id: 'desktop-pet-settings',
    section: 'desktop-pet',
    card: 'desktop-pet-settings',
    titleKey: 'config:desktopPetSettings.title',
    keywords: ['pet', '宠物', '桌宠', '宠物设置'],
  },
  {
    id: 'desktop-pet-install',
    section: 'desktop-pet',
    card: 'desktop-pet-install',
    titleKey: 'config:desktopPetSettings.installTitle',
    keywords: ['petdex', 'install', '宠物安装', '添加宠物'],
  },
  {
    id: 'desktop-pet-installed',
    section: 'desktop-pet',
    card: 'desktop-pet-installed',
    titleKey: 'config:desktopPetSettings.installedTitle',
    keywords: ['installed', '已安装', '宠物列表'],
  },
  // —— mobile-server 分区卡片 ——
  {
    id: 'mobile-server-main',
    section: 'mobile-server',
    card: 'mobile-server-main',
    titleKey: 'config:mobileServer.title',
    keywords: ['server', 'remote', '服务器', '远程', '手机访问'],
  },
  {
    id: 'mobile-server-local',
    section: 'mobile-server',
    card: 'mobile-server-local',
    titleKey: 'config:mobileServer.localTitle',
    keywords: ['local', 'on-device', '本机', '本地运行'],
  },
  // —— remote-access 分区卡片 ——
  {
    id: 'remote-access-main',
    section: 'remote-access',
    card: 'remote-access-main',
    titleKey: 'config:remoteAccess.title',
    keywords: ['remote', 'network', 'phone', '远程', '网络', '手机'],
  },
  {
    id: 'remote-access-pairing',
    section: 'remote-access',
    card: 'remote-access-pairing',
    titleKey: 'config:remoteAccess.pairing',
    keywords: ['pairing', 'qr', '配对', '二维码'],
  },
  {
    id: 'remote-access-devices',
    section: 'remote-access',
    card: 'remote-access-devices',
    titleKey: 'config:remoteAccess.devices',
    keywords: ['device', 'paired', '设备', '已配对', '授权'],
  },
  // —— updates 分区卡片 ——
  {
    id: 'updates-main',
    section: 'updates',
    card: 'updates-main',
    titleKey: 'config:updateSettings.pisperAppUpdates',
    keywords: ['update', 'version', 'channel', '更新', '版本', '升级', '渠道'],
  },
  {
    id: 'updates-components',
    section: 'updates',
    card: 'updates-components',
    titleKey: 'config:updateSettings.appComponents',
    keywords: ['component', 'runtime', 'sidecar', '组件', '运行时'],
  },
  {
    id: 'updates-cli',
    section: 'updates',
    card: 'updates-cli',
    titleKey: 'config:cliSettings.title',
    keywords: ['cli', 'terminal', 'command', '命令行', '终端', '命令'],
  },
  {
    id: 'updates-sponsors',
    section: 'updates',
    card: 'updates-sponsors',
    titleKey: 'config:updateSettings.sponsors',
    keywords: ['sponsor', '赞助', '赞助商'],
  },
  // —— about 分区卡片 ——
  {
    id: 'about-project',
    section: 'about',
    card: 'about-project',
    titleKey: 'config:aboutSettings.projectInformation',
    keywords: ['project', 'version', 'license', '项目', '版本', '许可'],
  },
  {
    id: 'about-support',
    section: 'about',
    card: 'about-support',
    titleKey: 'config:aboutSettings.supportPisper',
    keywords: ['support', 'donate', 'sponsor', '支持', '捐赠', '赞助'],
  },
]

// 常见口语别名 → 条目 id：整词（或查询中的分词）命中别名表时直接关联对应条目，
// 无需依赖标题/关键词的子串匹配。
const ALIAS_TABLE: Record<string, readonly string[]> = {
  provider: ['sec-models', 'models-current-model', 'models-connections'],
  供应商: ['sec-models', 'models-current-model', 'models-connections'],
  model: ['sec-models', 'models-current-model', 'models-connections'],
  模型: ['sec-models', 'models-current-model', 'models-connections'],
  theme: ['sec-interface', 'interface-appearance'],
  主题: ['sec-interface', 'interface-appearance'],
  interface: ['sec-interface', 'interface-appearance'],
  界面: ['sec-interface', 'interface-appearance', 'interface-language'],
  pet: ['sec-desktop-pet', 'desktop-pet-settings', 'desktop-pet-install', 'desktop-pet-installed'],
  宠物: ['sec-desktop-pet', 'desktop-pet-settings', 'desktop-pet-install', 'desktop-pet-installed'],
  update: ['sec-updates', 'updates-main', 'updates-components'],
  更新: ['sec-updates', 'updates-main', 'updates-components'],
  network: ['sec-remote-access', 'sec-mobile-server', 'remote-access-main'],
  网络: ['sec-remote-access', 'sec-mobile-server', 'remote-access-main'],
  remote: ['sec-remote-access', 'sec-mobile-server', 'remote-access-main'],
  远程: ['sec-remote-access', 'sec-mobile-server', 'remote-access-main'],
  notification: ['sec-notifications', 'notifications-browser', 'notifications-templates'],
  通知: ['sec-notifications', 'notifications-browser', 'notifications-templates'],
  language: ['interface-language'],
  语言: ['interface-language'],
}

const MAX_RESULTS = 12

function titlePair(entry: ConfigSearchEntry): [string, string] {
  return [
    translateText(entry.titleKey, 'zh-CN').toLowerCase(),
    translateText(entry.titleKey, 'en-US').toLowerCase(),
  ]
}

// 搜索设置索引：返回按相关度排序的条目（别名命中 > 标题精确/前缀 > 标题包含 > 关键词）。
export function searchConfig(query: string, language: SupportedLanguage): ConfigSearchMatch[] {
  const needle = query.trim().toLowerCase()
  if (!needle) return []

  // 别名表命中：先按整词查，再按空白分词查（支持「我的 宠物」这类组合查询）。
  const aliasHits = new Set<string>(ALIAS_TABLE[needle] || [])
  for (const token of needle.split(/\s+/)) {
    for (const id of ALIAS_TABLE[token] || []) aliasHits.add(id)
  }

  const scored: Array<{ entry: ConfigSearchEntry; score: number }> = []
  for (const entry of ENTRIES) {
    const [zh, en] = titlePair(entry)
    let score = 0
    if (aliasHits.has(entry.id)) score = 100
    else if (zh === needle || en === needle) score = 90
    else if (zh.startsWith(needle) || en.startsWith(needle)) score = 80
    else if (zh.includes(needle) || en.includes(needle)) score = 65
    else {
      // 关键词匹配：中文任意长度可匹配；英文至少 2 个字符，避免单字母噪声。
      const keywordScore = (entry.keywords || []).some((keyword) => {
        const lowered = keyword.toLowerCase()
        if (lowered.includes(needle)) return needle.length >= 2 || /[\u4e00-\u9fff]/.test(needle)
        return /[\u4e00-\u9fff]/.test(lowered) && lowered.includes(needle)
      })
      if (keywordScore) score = 40
    }
    if (score > 0) scored.push({ entry, score })
  }
  scored.sort((a, b) => b.score - a.score)

  // 同分区内展示标题相同的条目去重（如分区级「远程访问」与其首张卡片）：
  // 结果已按相关度排序，保留先出现的目标，避免下拉里出现两条同名结果。
  const seen = new Set<string>()
  const matches: ConfigSearchMatch[] = []
  for (const { entry } of scored) {
    const match: ConfigSearchMatch = {
      entry,
      title: translateText(entry.titleKey, language),
      sectionTitle: translateText(SECTION_TITLE_KEYS[entry.section], language),
    }
    const dedupeKey = `${entry.section}|${match.title}`
    if (seen.has(dedupeKey)) continue
    seen.add(dedupeKey)
    matches.push(match)
    if (matches.length >= MAX_RESULTS) break
  }
  return matches
}

// —— 目标卡片高亮桥接 ——
// 搜索框（PageHeader 内）与设置卡片（ConfigPage 内）分属不同组件树，
// 用模块级 pending + 自定义事件传递「点击结果 → 高亮卡片」请求；
// 分区切换后由 useConfigCardHighlight 在新 DOM 挂载后消费。
export const CONFIG_CARD_HIGHLIGHT_EVENT = 'pisper:config-card-highlight'

type PendingHighlight = {
  card: string
  section: string
}

let pendingHighlight: PendingHighlight | null = null

export function requestConfigCardHighlight(card: string, section: string) {
  pendingHighlight = { card, section }
  window.dispatchEvent(new Event(CONFIG_CARD_HIGHLIGHT_EVENT))
}

function takePendingHighlight(): PendingHighlight | null {
  const pending = pendingHighlight
  pendingHighlight = null
  return pending
}

// 对目标卡片做 1.2s 高亮闪烁：平滑滚动到视口中央，用现有焦点 token
// （--focus / --focus-ring）临时加强 ring 与描边，到点后还原原内联样式。
function flashConfigCard(pending: PendingHighlight) {
  let target = document.querySelector<HTMLElement>(`[data-config-card="${pending.card}"]`)
  // 目标卡片不存在（如条件渲染的卡片）时回退到分区根容器，保证点击总有反馈。
  if (!target)
    target = document.querySelector<HTMLElement>(`[data-config-card="${CONFIG_SECTION_ANCHOR}"]`)
  if (!target) return
  target.scrollIntoView({ behavior: 'smooth', block: 'center' })
  const previous = {
    boxShadow: target.style.boxShadow,
    borderColor: target.style.borderColor,
    transition: target.style.transition,
  }
  target.style.transition = 'box-shadow .3s ease, border-color .3s ease'
  target.style.boxShadow = '0 0 0 3px var(--focus-ring)'
  target.style.borderColor = 'var(--focus)'
  window.setTimeout(() => {
    target.style.boxShadow = previous.boxShadow
    target.style.borderColor = previous.borderColor
    target.style.transition = previous.transition
  }, 1200)
}

// ConfigPage 挂载/分区切换时消费待处理的高亮请求。
// 同分区点击由事件监听即时消费；跨分区点击在新区内容渲染后由本效应消费。
export function useConfigCardHighlight(section: string) {
  useEffect(() => {
    const consume = () => {
      const pending = takePendingHighlight()
      if (!pending) return
      // 双 rAF：等分区切换后的新卡片完成布局再查找锚点。
      requestAnimationFrame(() => requestAnimationFrame(() => flashConfigCard(pending)))
    }
    consume()
    window.addEventListener(CONFIG_CARD_HIGHLIGHT_EVENT, consume)
    return () => window.removeEventListener(CONFIG_CARD_HIGHLIGHT_EVENT, consume)
  }, [section])
}
