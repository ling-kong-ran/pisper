// Provider 多 Key 私有存储：auth.json 仍保留主 Key 供 Pi 原生兼容，
// 其余 Key 单独以 0600 保存，并只向普通配置视图暴露不可逆标识和掩码。
import { createHash } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

const MAX_KEYS_PER_PROVIDER = 32
const MAX_KEY_LENGTH = 16_384

function keyId(value) {
  return createHash('sha256').update(value).digest('hex').slice(0, 24)
}

function cleanKey(value) {
  const key = String(value || '').trim()
  if (!key) return ''
  if (key.length > MAX_KEY_LENGTH) throw new Error('API Key 过长。')
  return key
}

function uniqueKeys(values) {
  const seen = new Set()
  const result = []
  for (const value of Array.isArray(values) ? values : [values]) {
    const key = cleanKey(value)
    if (!key || seen.has(key)) continue
    seen.add(key)
    result.push(key)
  }
  if (result.length > MAX_KEYS_PER_PROVIDER) {
    throw new Error(`单个 Provider 最多保存 ${MAX_KEYS_PER_PROVIDER} 个 API Key。`)
  }
  return result
}

function safeRecord(value) {
  const key = cleanKey(value?.key)
  return key ? { id: keyId(key), key } : null
}

function maskKey(key) {
  if (key.length <= 8) return '********'
  return `${key.slice(0, 3)}...${key.slice(-4)}`
}

export class ProviderKeyringService {
  constructor({ path }) {
    this.path = path
    this.queue = Promise.resolve()
  }

  async readAll() {
    const data = await readJson(this.path, { version: 1, providers: {} })
    return data && typeof data === 'object' && data.providers && typeof data.providers === 'object'
      ? data
      : { version: 1, providers: {} }
  }

  async records(providerId, primaryKey = '') {
    const data = await this.readAll()
    const configured = Array.isArray(data.providers?.[providerId])
      ? data.providers[providerId].map(safeRecord).filter(Boolean)
      : []
    const legacy = cleanKey(primaryKey)
    if (legacy && !configured.some((record) => record.key === legacy)) {
      configured.unshift({ id: keyId(legacy), key: legacy })
    }
    return configured.slice(0, MAX_KEYS_PER_PROVIDER)
  }

  async keys(providerId, primaryKey = '') {
    return (await this.records(providerId, primaryKey)).map((record) => record.key)
  }

  async summaries(providerId, primaryKey = '') {
    return (await this.records(providerId, primaryKey)).map((record) => ({
      id: record.id,
      hint: maskKey(record.key),
    }))
  }

  async add(providerId, values, primaryKey = '') {
    const incoming = uniqueKeys(values)
    const existing = await this.records(providerId, primaryKey)
    const merged = [...existing]
    for (const key of incoming) {
      if (!merged.some((record) => record.key === key)) merged.push({ id: keyId(key), key })
    }
    if (merged.length > MAX_KEYS_PER_PROVIDER) {
      throw new Error(`单个 Provider 最多保存 ${MAX_KEYS_PER_PROVIDER} 个 API Key。`)
    }
    this.queue = this.queue
      .catch(() => {})
      .then(async () => {
        const data = await this.readAll()
        data.providers ||= {}
        data.providers[providerId] = merged
        await writeJsonAtomic(this.path, data, { mode: 0o600 })
      })
    await this.queue
    return merged
  }

  async mergeSelected(snapshot, providerIds) {
    const selected = new Set(Array.isArray(providerIds) ? providerIds.map(String) : [])
    this.queue = this.queue
      .catch(() => {})
      .then(async () => {
        const data = await this.readAll()
        data.providers ||= {}
        for (const providerId of selected) {
          const records = snapshot?.providers?.[providerId]
          const keys = uniqueKeys(
            (Array.isArray(records) ? records : []).map((record) => record?.key),
          )
          if (keys.length) data.providers[providerId] = keys.map((key) => ({ id: keyId(key), key }))
          else delete data.providers[providerId]
        }
        await writeJsonAtomic(this.path, data, { mode: 0o600 })
      })
    await this.queue
  }

  async replace(providerId, values) {
    const keys = uniqueKeys(values)
    this.queue = this.queue
      .catch(() => {})
      .then(async () => {
        const data = await this.readAll()
        data.providers ||= {}
        if (keys.length) data.providers[providerId] = keys.map((key) => ({ id: keyId(key), key }))
        else delete data.providers[providerId]
        await writeJsonAtomic(this.path, data, { mode: 0o600 })
      })
    await this.queue
    return keys.map((key) => ({ id: keyId(key), key }))
  }

  async remove(providerId) {
    this.queue = this.queue
      .catch(() => {})
      .then(async () => {
        const data = await this.readAll()
        if (!data.providers?.[providerId]) return
        delete data.providers[providerId]
        await writeJsonAtomic(this.path, data, { mode: 0o600 })
      })
    await this.queue
  }

  id(value) {
    return keyId(cleanKey(value))
  }
}
