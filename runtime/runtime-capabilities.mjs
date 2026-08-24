// 同一套 Pisper Runtime 在不同宿主上使用这份能力清单做降级。能力只能由实际
// 内建模块探测开启；环境变量只选择宿主档案，不能把不存在的能力伪装成可用。
const PROFILES = new Set(['desktop', 'mobile-root', 'mobile-embedded', 'mobile-store'])

function requestedProfile(environment) {
  const value = String(environment.PISPER_RUNTIME_PROFILE || '').trim()
  return PROFILES.has(value) ? value : 'desktop'
}

async function supportsModule(specifier) {
  try {
    await import(specifier)
    return true
  } catch {
    return false
  }
}

function supportedTools(features) {
  const tools = new Set([
    'read',
    'ls',
    'edit',
    'write',
    'skill_create',
    'web_search',
    // 设备操作由当前手机 App 执行，不依赖 Node 宿主的内建模块。
    'mobile_device',
  ])
  if (features.processes) {
    tools.add('grep')
    tools.add('find')
    tools.add('bash')
  }
  if (features.memory) {
    tools.add('memory_search')
    tools.add('memory_remember')
  }
  if (features.mcp) {
    tools.add('mcp_list')
    tools.add('mcp_manage')
  }
  if (features.plugins) tools.add('plugin_create')
  if (features.browserAutomation) tools.add('browser_automation')
  if (features.visualGeneration) tools.add('generate_visual')
  return [...tools].sort()
}

function buildCapabilities({ profile, childProcess, workerThreads, sqlite, wasm }) {
  const store = profile === 'mobile-store'
  const embedded = profile === 'mobile-embedded' || store
  const processes = childProcess && !store
  const workers = workerThreads && !store
  const features = {
    chat: true,
    sessions: true,
    providers: true,
    filesystem: true,
    assets: true,
    skills: true,
    webSearch: true,
    visualGeneration: true,
    imageProcessing: wasm,
    processes,
    shell: processes,
    terminal: processes && profile === 'desktop',
    vcs: processes,
    memory: sqlite,
    workers,
    plugins: workers && !embedded,
    mcp: processes && !embedded,
    goals: !embedded,
    plans: !embedded,
    multiAgent: !embedded,
    channels: !embedded,
    workflows: !embedded,
    schedules: !embedded,
    browserAutomation: profile === 'desktop',
    remoteAccess: profile === 'desktop',
    desktopPet: profile === 'desktop',
  }
  return Object.freeze({
    version: 1,
    profile,
    engine: 'node',
    degraded: Object.values(features).some((available) => !available),
    modules: Object.freeze({ childProcess, workerThreads, sqlite, wasm }),
    features: Object.freeze(features),
    tools: Object.freeze(supportedTools(features)),
  })
}

export function desktopRuntimeCapabilities() {
  return buildCapabilities({
    profile: 'desktop',
    childProcess: true,
    workerThreads: true,
    sqlite: true,
    wasm: true,
  })
}

export async function resolveRuntimeCapabilities({
  environment = process.env,
  moduleSupport,
} = {}) {
  const profile = requestedProfile(environment)
  const detected = moduleSupport || {
    childProcess: await supportsModule('node:child_process'),
    workerThreads: await supportsModule('node:worker_threads'),
    sqlite: await supportsModule('node:sqlite'),
    wasm: typeof WebAssembly === 'object',
  }
  return buildCapabilities({
    profile,
    childProcess: detected.childProcess === true,
    workerThreads: detected.workerThreads === true,
    sqlite: detected.sqlite === true,
    wasm: detected.wasm === true,
  })
}
