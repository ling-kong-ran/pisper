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
  | 'browserAutomation'
  | 'remoteAccess'
  | 'desktopPet'

export type RuntimeCapabilities = {
  version: number
  profile: 'desktop' | 'mobile-root' | 'mobile-embedded'
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
  browserAutomation: true,
  remoteAccess: true,
  desktopPet: true,
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
