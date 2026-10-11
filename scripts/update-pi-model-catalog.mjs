import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { format } from 'prettier'
import { download, sha256 } from './upstream-download.mjs'

export const PI_CATALOG_URL = 'https://pi.dev/api/models?types=chat,image,classifier'
export const PI_CATALOG_PATH = 'runtime-rs/assets/pi-model-catalog.json'
const record = (value) => value && typeof value === 'object' && !Array.isArray(value)
const limit = (value) => Number.isSafeInteger(value) && value > 0 && value <= 100_000_000

export function validateCatalog(models) {
  if (!record(models) || !Object.keys(models).length) throw new Error('Empty Pi model catalog')
  let chats = 0
  for (const [provider, entries] of Object.entries(models)) {
    if (
      !/^[a-z0-9][a-z0-9._-]{0,127}$/.test(provider) ||
      !Array.isArray(entries) ||
      !entries.length
    )
      throw new Error('Invalid Pi provider catalog')
    const ids = new Set()
    for (const model of entries) {
      if (
        !record(model) ||
        typeof model.id !== 'string' ||
        !model.id ||
        model.id.length > 512 ||
        ids.has(`${model.type}:${model.id}`) ||
        model.provider !== provider ||
        typeof model.api !== 'string' ||
        !model.api ||
        !['chat', 'image', 'classifier'].includes(model.type)
      )
        throw new Error('Invalid Pi model identity or type')
      ids.add(`${model.type}:${model.id}`)
      if (model.type !== 'chat') continue
      chats++
      if (
        !limit(model.contextWindow) ||
        !limit(model.maxTokens) ||
        typeof model.reasoning !== 'boolean' ||
        !Array.isArray(model.input) ||
        !model.input.length ||
        !record(model.cost) ||
        !['input', 'output', 'cacheRead', 'cacheWrite'].every(
          (key) =>
            Number.isFinite(model.cost[key]) &&
            (model.cost[key] >= 0 || model.cost[key] === -1000000),
        )
      )
        throw new Error('Invalid Pi chat model capabilities')
      if (model.compat !== undefined && !record(model.compat))
        throw new Error('Invalid Pi model compatibility')
      if (
        model.thinkingLevelMap !== undefined &&
        (!record(model.thinkingLevelMap) ||
          !Object.values(model.thinkingLevelMap).every(
            (value) => value === null || typeof value === 'string',
          ))
      )
        throw new Error('Invalid Pi thinking levels')
      // Bedrock/Azure 等端点在配置连接时解析，官方目录允许空地址。
      if (typeof model.baseUrl !== 'string') throw new Error('Invalid Pi model endpoint')
      if (model.baseUrl) {
        const url = new URL(model.baseUrl)
        if (url.protocol !== 'https:' || url.username || url.password)
          throw new Error('Invalid Pi model endpoint')
      }
    }
  }
  if (!chats) throw new Error('Pi catalog contains no chat models')
  return models
}

export async function updatePiModelCatalog({
  root = resolve(import.meta.dirname, '..'),
  fetchImpl = fetch,
} = {}) {
  const { bytes, headers } = await download(PI_CATALOG_URL, { fetchImpl })
  const revision = headers.get('x-pi-model-catalog-revision')
  if (!/^sha256-[a-f0-9]{64}$/.test(revision || '') || revision !== `sha256-${sha256(bytes)}`)
    throw new Error('Pi catalog revision checksum mismatch')
  const models = validateCatalog(
    JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)),
  )
  const snapshot = {
    schemaVersion: 1,
    source: {
      url: PI_CATALOG_URL,
      revision,
      license: 'MIT',
      licenseUrl: 'https://github.com/earendil-works/pi/blob/main/LICENSE',
    },
    models,
  }
  const content = await format(JSON.stringify(snapshot), { parser: 'json', printWidth: 100 })
  const destination = resolve(root, PI_CATALOG_PATH)
  const previous = await readFile(destination, 'utf8').catch((error) => {
    if (error.code === 'ENOENT') return null
    throw error
  })
  if (previous === content) return { changed: false, revision }
  await mkdir(dirname(destination), { recursive: true })
  const temporary = `${destination}.${process.pid}.tmp`
  try {
    await writeFile(temporary, content, { flag: 'wx' })
    await rename(temporary, destination)
  } finally {
    await rm(temporary, { force: true })
  }
  return { changed: true, revision }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const result = await updatePiModelCatalog()
  console.log(`${result.changed ? 'Updated' : 'Unchanged'} Pi model catalog: ${result.revision}`)
}
