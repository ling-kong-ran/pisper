import { rm, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'
import { readJson, writeJsonAtomic } from './storage/json-file.mjs'

export const CURRENT_HOME = '.pisper'

export function resolveAgentDataDir({ env = process.env, home = homedir() } = {}) {
  const explicit = env.PISPER_AGENT_DIR
  return explicit ? resolve(explicit) : join(home, CURRENT_HOME, 'agent')
}

export async function cleanupRemovedLocalEmbeddingData(dataDir) {
  const root = resolve(dataDir)
  const configPath = join(root, 'pisper.json')
  const config = await readJson(configPath, null)
  let configUpdated = false
  if (config && typeof config === 'object' && Object.hasOwn(config, 'memoryEmbedding')) {
    delete config.memoryEmbedding
    await writeJsonAtomic(configPath, config)
    configUpdated = true
  }

  const modelsPath = join(root, 'pisper-memory-models')
  const modelsRemoved = Boolean(
    await stat(modelsPath).catch((error) => {
      if (error?.code === 'ENOENT') return null
      throw error
    }),
  )
  if (modelsRemoved) await rm(modelsPath, { recursive: true, force: true })
  return { configUpdated, modelsRemoved }
}
