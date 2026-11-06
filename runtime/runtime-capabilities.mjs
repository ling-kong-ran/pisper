import { BETA_FEATURES } from '../shared/beta-features.mjs'

// 同一套 Pisper Runtime 在不同宿主上使用这份能力清单做降级。能力只能由实际
// 内建模块探测开启；环境变量只选择宿主档案，不能把不存在的能力伪装成可用。
const PROFILES = new Set(['desktop', 'mobile-embedded', 'mobile-store'])

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
    // 原生组件不需要编译器，三个档案都能写组件（components.native 恒为 true）。
    'component_create',
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
    tools.add('memory_open')
    tools.add('memory_confirm')
    tools.add('memory_forget')
  }
  if (features.mcp) {
    tools.add('mcp_list')
    tools.add('mcp_manage')
  }
  if (features.plugins) tools.add('plugin_create')
  if (features.schedules) {
    tools.add('schedule_list')
    tools.add('schedule_manage')
  }
  if (features.workflows) {
    tools.add('workflow_list')
    tools.add('workflow_run')
  }
  if (features.browserAutomation) tools.add('browser_automation')
  if (features.visualGeneration) tools.add('generate_visual')
  if (features.imageAssets) tools.add('image_assets')
  if (features.codemode) tools.add('codemode')
  return [...tools].sort()
}

function buildCapabilities({
  profile,
  childProcess,
  workerThreads,
  sqlite,
  wasm,
  processSpawning,
}) {
  const store = profile === 'mobile-store'
  const embedded = profile === 'mobile-embedded' || store
  const processes = childProcess && processSpawning && !store
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
    imageAssets: workers && wasm,
    processes,
    shell: processes,
    terminal: processes && profile === 'desktop',
    vcs: processes,
    memory: sqlite,
    workers,
    plugins: workers && !embedded,
    extensions: true,
    mcp: processes && !embedded,
    goals: !embedded,
    plans: !embedded,
    multiAgent: !embedded,
    channels: !embedded,
    workflows: !embedded,
    schedules: !embedded,
    computerUse: profile === 'desktop',
    browserAutomation: profile === 'desktop',
    remoteAccess: profile === 'desktop',
    desktopPet: profile === 'desktop',
    // 看板视图与事件流只依赖内存中的会话摘要，桌面与两种手机档案都提供。
    'board.view': true,
    'board.events': true,
    // ask_user（等你回答）：只依赖内存里的待回答队列，各档案都提供。
    askUser: true,
    // 原生组件（A7）：同源 ESM 模块 + JSON 文件存储，不需要编译器或原生二进制，三种档案都提供。
    'components.native': true,
    // 排在 X 之后启动：队列落盘在数据目录，各档案都提供。
    sessionQueue: true,
    // 会话导出 Markdown：只读已投影的转录。
    sessionExport: true,
    // 新建项目文件夹：只建一层空目录，各档案都提供。
    workspaceFolders: true,
    // 插队（立即发送）：/input 的 behavior: 'interrupt' 与排队项的 send-now，只打断当前这一步。
    inputInterrupt: true,
    // 实验功能开关：只读写应用配置里的一个字段，各档案都提供；单项功能是否可用另看它 requires 的能力。
    // 清单为空时不声明：设置里不出现空的“实验功能”分区，等第一个实验功能进清单再出现。
    betaFeatures: BETA_FEATURES.length > 0,
    // Bots（B1 起）：依赖渠道、定时任务与本机工作目录，只在桌面档案提供。
    bots: !embedded,
    // 远程机器：SSH 要拉起系统的 ssh，执行节点要连回桌面端的远程访问；手机端不能发起，连到桌面端时用桌面端配好的。
    remoteMachines: processes && !embedded,
    // codemode 脚本跑在 worker + QuickJS(wasm) 沙箱；手机端未验证（iOS 嵌入式 Node 无 JIT），先不提供。
    codemode: workers && wasm && !embedded,
  }
  return Object.freeze({
    version: 1,
    profile,
    engine: 'node',
    // 实验功能清单为空不是宿主降级，不计入。
    degraded: Object.entries(features).some(
      ([name, available]) => !available && name !== 'betaFeatures',
    ),
    modules: Object.freeze({ childProcess, workerThreads, sqlite, wasm }),
    features: Object.freeze(features),
    tools: Object.freeze(supportedTools(features)),
  })
}

// 分阶段启动必须显式启用，不能让桌面因环境探测意外改变就绪语义。
export function mobileBaseInitialization(mode, profile) {
  if (mode === 'full') return false
  if (mode !== 'mobile-base') throw new Error(`Unknown initialization mode: ${mode}`)
  if (!PROFILES.has(profile) || !profile.startsWith('mobile-'))
    throw new Error('mobile-base initialization requires a mobile runtime profile')
  return true
}

export function desktopRuntimeCapabilities() {
  return buildCapabilities({
    profile: 'desktop',
    childProcess: true,
    processSpawning: true,
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
  // iOS 的 NodeMobile 可以导入 child_process，但系统禁止 App 创建子进程；不能把模块存在误当成 spawn 可用。
  const processSpawning = environment.PISPER_RUNTIME_PLATFORM !== 'ios'
  return buildCapabilities({
    profile,
    childProcess: detected.childProcess === true,
    processSpawning,
    workerThreads: detected.workerThreads === true,
    sqlite: detected.sqlite === true,
    wasm: detected.wasm === true,
  })
}
