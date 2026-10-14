// 数据目录解析与旧数据清理：集中处理 .pisper 目录的定位，以及历史版本遗留数据的移除。
import { rm, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'
import { readJson, writeJsonAtomic } from './storage/json-file.mjs'

export const CURRENT_HOME = '.pisper'

// 数据目录优先级：显式环境变量 > 用户主目录下的默认位置。
// 桌面/开发/测试都走这里，避免各处重复实现路径拼接逻辑。
export function resolveAgentDataDir({ env = process.env, home = homedir() } = {}) {
  const explicit = env.PISPER_AGENT_DIR
  return explicit ? resolve(explicit) : join(home, CURRENT_HOME, 'agent')
}

// 清理旧版本地嵌入（embedding）数据：历史版本曾把模型与配置写入数据目录，
// 该功能下线后需要把残留配置项和模型目录一并移除，避免磁盘占用与误导性配置。
export async function cleanupRemovedLocalEmbeddingData(dataDir) {
  const root = resolve(dataDir)
  const configPath = join(root, 'pisper.json')
  const config = await readJson(configPath, null)
  let configUpdated = false
  // 移除记忆嵌入相关配置键；文件不存在时 readJson 返回 null，无需特殊处理。
  if (config && typeof config === 'object' && Object.hasOwn(config, 'memoryEmbedding')) {
    delete config.memoryEmbedding
    await writeJsonAtomic(configPath, config)
    configUpdated = true
  }

  // 删除模型目录：stat 探测存在性，ENOENT 视为不存在而非错误。
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
