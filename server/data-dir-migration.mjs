import { cpSync, existsSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'

// Pisper（原 Vesper）改名后，用户数据默认目录从 ~/.vesper 迁到 ~/.pisper。
// 显式配置 PISPER_AGENT_DIR / VESPER_AGENT_DIR 时不做迁移，尊重用户选择。

export const LEGACY_HOME = '.vesper'
export const CURRENT_HOME = '.pisper'
const MERGE_MARKER = '.vesper-data-merged'

function readJsonFile(path) {
  try {
    return JSON.parse(readFileSync(path, 'utf8'))
  } catch {
    return null
  }
}

function mergeJsonFile(currentPath, legacyPath, merge) {
  const legacy = readJsonFile(legacyPath)
  if (!legacy || typeof legacy !== 'object' || Array.isArray(legacy)) return false
  const current = readJsonFile(currentPath)
  const merged = merge(legacy, current && typeof current === 'object' && !Array.isArray(current) ? current : {})
  writeFileSync(currentPath, `${JSON.stringify(merged, null, 2)}\n`)
  return true
}

function mergeLegacyProviderData(legacyAgent, currentAgent) {
  const paths = (filename) => [join(currentAgent, filename), join(legacyAgent, filename)]

  const [currentModels, legacyModels] = paths('models.json')
  mergeJsonFile(currentModels, legacyModels, (legacy, current) => ({
    ...legacy,
    ...current,
    providers: { ...(legacy.providers || {}), ...(current.providers || {}) },
  }))

  const [currentAuth, legacyAuth] = paths('auth.json')
  mergeJsonFile(currentAuth, legacyAuth, (legacy, current) => ({ ...legacy, ...current }))

  const [currentConfig, legacyConfig] = paths('pisper.json')
  mergeJsonFile(currentConfig, legacyConfig, (legacy, current) => ({
    ...legacy,
    ...current,
    disabledProviders: Array.isArray(current.disabledProviders)
      ? current.disabledProviders
      : (legacy.disabledProviders || []),
    providerTypes: { ...(legacy.providerTypes || {}), ...(current.providerTypes || {}) },
    providerDefaultModels: { ...(legacy.providerDefaultModels || {}), ...(current.providerDefaultModels || {}) },
  }))
}

export function resolveAgentDataDir({
  env = process.env,
  home = homedir(),
  log = console.warn,
} = {}) {
  const explicit = env.PISPER_AGENT_DIR || env.VESPER_AGENT_DIR
  if (explicit) return resolve(explicit)

  const currentHome = join(home, CURRENT_HOME)
  const legacyHome = join(home, LEGACY_HOME)
  const currentAgent = join(currentHome, 'agent')
  const legacyAgent = join(legacyHome, 'agent')

  if (!existsSync(currentAgent) && existsSync(legacyAgent)) {
    try {
      renameSync(legacyHome, currentHome)
      log?.(`[pisper] 已将数据目录从 ${legacyHome} 迁移到 ${currentHome}`)
    } catch (renameError) {
      // 跨设备/权限导致 rename 失败时退化为复制，成功后删除旧目录。
      try {
        cpSync(legacyHome, currentHome, { recursive: true, errorOnExist: false, force: false })
        rmSync(legacyHome, { recursive: true, force: true })
        log?.(`[pisper] 已将数据目录从 ${legacyHome} 复制迁移到 ${currentHome}`)
      } catch (copyError) {
        // 迁移失败不影响启动：回退旧目录，保证用户数据可用。
        log?.(`[pisper] 数据目录迁移失败，继续使用 ${legacyHome}`, copyError || renameError)
        return legacyAgent
      }
    }
  } else if (existsSync(currentAgent) && existsSync(legacyAgent) && !existsSync(join(currentAgent, MERGE_MARKER))) {
    try {
      // 保留新目录已写入的数据，同时补齐旧目录中的会话、资源等缺失文件。
      cpSync(legacyAgent, currentAgent, { recursive: true, errorOnExist: false, force: false })
      // Provider 配置需要按 ID 合并；仅复制 models.json 会丢失其中一侧的连接。
      mergeLegacyProviderData(legacyAgent, currentAgent)
      // 旧目录保留作备份；标记确保以后删除的 Provider 不会被旧数据重新导入。
      writeFileSync(join(currentAgent, MERGE_MARKER), 'merged\n')
      log?.(`[pisper] 已合并 ${legacyAgent} 中的 Provider 配置到 ${currentAgent}`)
    } catch (error) {
      // 新目录仍可用；合并失败不能阻止启动或覆盖用户当前数据。
      log?.(`[pisper] 数据目录合并失败，继续使用 ${currentAgent}`, error)
    }
  }

  return currentAgent
}
