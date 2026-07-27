import { cpSync, existsSync, renameSync, rmSync } from 'node:fs'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'

// Pisper（原 Vesper）改名后，用户数据默认目录从 ~/.vesper 迁到 ~/.pisper。
// 显式配置 PISPER_AGENT_DIR / VESPER_AGENT_DIR 时不做迁移，尊重用户选择。

export const LEGACY_HOME = '.vesper'
export const CURRENT_HOME = '.pisper'

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

  // 新目录已存在：什么都不做，旧目录留作备份，绝不覆盖新数据。
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
  }

  return currentAgent
}
