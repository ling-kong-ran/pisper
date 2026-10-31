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
