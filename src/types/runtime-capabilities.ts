export type RuntimeFeature =
  | 'chat'
  | 'sessions'
  | 'providers'
  | 'filesystem'
  | 'assets'
  | 'skills'
  | 'webSearch'
  | 'visualGeneration'
  | 'imageProcessing'
  | 'imageAssets'
  | 'processes'
  | 'shell'
  | 'terminal'
  | 'vcs'
  | 'memory'
  | 'workers'
  | 'plugins'
  | 'mcp'
  | 'goals'
  | 'plans'
  | 'multiAgent'
  | 'channels'
  | 'workflows'
  | 'schedules'
  | 'computerUse'
  | 'browserAutomation'
  | 'remoteAccess'
  | 'desktopPet'
  | 'board.view'
  | 'board.events'
  | 'askUser'
  | 'components.native'
  | 'sessionQueue'
  | 'sessionExport'

export type RuntimeCapabilities = {
  version: number
  profile: 'desktop' | 'mobile-embedded' | 'mobile-store'
  engine: 'node'
  degraded: boolean
  modules: {
    childProcess: boolean
    workerThreads: boolean
    sqlite: boolean
    wasm: boolean
  }
  features: Record<RuntimeFeature, boolean>
  tools: string[]
}

const fullFeatures = {
  chat: true,
  sessions: true,
  providers: true,
  filesystem: true,
  assets: true,
  skills: true,
  webSearch: true,
  visualGeneration: true,
  imageProcessing: true,
  imageAssets: true,
  processes: true,
  shell: true,
  terminal: true,
  vcs: true,
  memory: true,
  workers: true,
  plugins: true,
  mcp: true,
  goals: true,
  plans: true,
  multiAgent: true,
  channels: true,
  workflows: true,
  schedules: true,
  computerUse: true,
  browserAutomation: true,
  remoteAccess: true,
  desktopPet: true,
  // 第 2 期才有的看板接口：没有能力接口的旧 Runtime 一定不支持，按降级矩阵退回轮询 /api/sessions。
  'board.view': false,
  'board.events': false,
  // 方向 A 的新能力（规格 8.4、A9）：旧 Runtime 没有声明时前端永不出现“等你回答”、排队与导出。
  askUser: false,
  sessionQueue: false,
  sessionExport: false,
  // 原生组件（A7）：旧 Runtime 只有 iframe 组件，没有声明时组件页按旧版显示“不再支持”，宿主不请求实例。
  'components.native': false,
} satisfies Record<RuntimeFeature, boolean>

// 老版本 Runtime 没有能力接口；保持原有全功能导航，避免升级前的远程桌面被误裁剪。
export const LEGACY_RUNTIME_CAPABILITIES: RuntimeCapabilities = {
  version: 0,
  profile: 'desktop',
  engine: 'node',
  degraded: false,
  modules: { childProcess: true, workerThreads: true, sqlite: true, wasm: true },
  features: fullFeatures,
  tools: [],
}

export function runtimeFeatureAvailable(
  capabilities: RuntimeCapabilities,
  feature: RuntimeFeature,
) {
  return capabilities.features[feature] !== false
}

const PAGE_FEATURES: Partial<Record<string, RuntimeFeature>> = {
  assets: 'assets',
  channels: 'channels',
  schedules: 'schedules',
  memory: 'memory',
  mcp: 'mcp',
  skills: 'skills',
  workflows: 'workflows',
  workflowCreate: 'workflows',
}

const CONFIG_FEATURES: Partial<Record<string, RuntimeFeature>> = {
  'desktop-pet': 'desktopPet',
  'remote-access': 'remoteAccess',
}

export function runtimePageAvailable(capabilities: RuntimeCapabilities, page: string) {
  const feature = PAGE_FEATURES[page]
  return !feature || runtimeFeatureAvailable(capabilities, feature)
}

export function runtimeConfigSectionAvailable(capabilities: RuntimeCapabilities, section: string) {
  const feature = CONFIG_FEATURES[section]
  return !feature || runtimeFeatureAvailable(capabilities, feature)
}

// 功能 › 设备与连接（规格 2.5、2.7）的平台判断集中在这里：功能索引的卡片与三个页面本身读同一份结论。
// 远程访问只在桌面（手机 App 换成“手机服务器”）；桌面宠物只在桌面；Runtime 裁掉能力时为 runtime。
// 卡片按 runtime 隐藏、按 desktop-only / mobile-only 显示为禁用并写明原因；页面在不支持时显示禁用行与原因，不去调用接口。
export type DeviceFeatureSection = 'remote-access' | 'mobile-server' | 'desktop-pet'
export type DeviceFeatureSupport =
  { supported: true } | { supported: false; reason: 'desktop-only' | 'mobile-only' | 'runtime' }

export function deviceFeatureSupport(
  capabilities: RuntimeCapabilities,
  section: DeviceFeatureSection,
  { mobileApp }: { mobileApp: boolean },
): DeviceFeatureSupport {
  if (section === 'mobile-server')
    return mobileApp ? { supported: true } : { supported: false, reason: 'mobile-only' }
  if (mobileApp) return { supported: false, reason: 'desktop-only' }
  return runtimeConfigSectionAvailable(capabilities, section)
    ? { supported: true }
    : { supported: false, reason: 'runtime' }
}
