// 资产存储：管理资产目录中的文件资产（上传/生成/链接），维护资产索引 JSON，
// 提供存储、去重（内容哈希）、归档生成文件、会话归属与孤儿资产对账。
import { createHash, randomUUID } from 'node:crypto'
import { createReadStream } from 'node:fs'
import { copyFile, stat, unlink, writeFile } from 'node:fs/promises'
import { basename, extname, join, resolve, sep } from 'node:path'

async function hashFile(filePath) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(filePath)) hash.update(chunk)
  return hash.digest('hex')
}

function assetReferences(asset) {
  return Array.isArray(asset.references) ? asset.references : []
}

// 对公开 API 隐藏内部字段（存储路径/引用关系）。
export function publicAsset(asset) {
  if (!asset) return null
  const publicValue = { ...asset }
  delete publicValue.storagePath
  delete publicValue.references
  return publicValue
}

function absolutePath(value) {
  return typeof value === 'string' && value ? resolve(value) : null
}

function managedAssetPath(value, assetsDir) {
  const target = absolutePath(value)
  if (!target) return null
  const root = resolve(assetsDir)
  return target !== root && target.startsWith(`${root}${sep}`) ? target : null
}

async function readablePath(value) {
  const path = absolutePath(value)
  if (!path) return null
  const info = await stat(path).catch(() => null)
  return info?.isFile() ? { path, info } : null
}

async function firstReadablePath(values) {
  for (const value of values) {
    const readable = await readablePath(value)
    if (readable) return readable
  }
  return null
}

function hashBuffer(buffer) {
  return createHash('sha256').update(buffer).digest('hex')
}

export async function findContentDuplicate(assets, hash) {
  for (const asset of assets) {
    if (asset.kind === 'link' || asset.hash !== hash || !asset.storagePath) continue
    const storedInfo = await stat(asset.storagePath).catch(() => null)
    if (storedInfo?.isFile()) return asset
  }
  return null
}

export function addAssetReference(asset, input) {
  const reference = {
    source: String(input.source || 'upload'),
    sessionId: String(input.sessionId || ''),
    sessionName: String(input.sessionName || ''),
    name: String(input.name || asset.name || ''),
    kind: String(input.kind || asset.kind || 'file'),
    mimeType: String(input.mimeType || asset.mimeType || 'application/octet-stream'),
    size: Number(input.size) || 0,
    created: String(input.created || new Date().toISOString()),
  }
  if (input.filePath) reference.filePath = resolve(input.filePath)
  if (!Array.isArray(asset.references)) asset.references = []
  asset.references.push(reference)
  return reference
}

function mergeAssetReferences(target, source) {
  addAssetReference(target, source)
  for (const reference of assetReferences(source)) addAssetReference(target, reference)
}

export function assetForSession(asset, sessionId) {
  const reference = assetReferences(asset).findLast((item) => item.sessionId === sessionId)
  if (!reference) return asset.sessionId === sessionId ? asset : null
  return {
    ...asset,
    source: reference.source,
    sessionId: reference.sessionId,
    sessionName: reference.sessionName,
  }
}

export function assetHasSource(asset, source) {
  return (
    asset.source === source ||
    assetReferences(asset).some((reference) => reference.source === source)
  )
}

export function findAssetByFilePath(assets, filePath) {
  const sourcePath = resolve(filePath)
  let matched = null
  let matchedAt = -Infinity
  for (const asset of assets) {
    const occurrences = [
      asset,
      ...assetReferences(asset).map((reference) => ({ ...asset, ...reference })),
    ]
    for (const occurrence of occurrences) {
      if (!occurrence.filePath || resolve(occurrence.filePath) !== sourcePath) continue
      const createdAt = new Date(occurrence.created || 0).getTime()
      if (createdAt >= matchedAt) {
        matched = occurrence
        matchedAt = createdAt
      }
    }
  }
  return matched
}

export function generatedAssetsForSession(assets, sessionId) {
  return assets
    .flatMap((asset) => [
      asset,
      ...assetReferences(asset).map((reference) => ({ ...asset, ...reference })),
    ])
    .filter(
      (asset) =>
        asset.sessionId === sessionId &&
        asset.source === 'agent' &&
        /^(?:image|video)\//.test(asset.mimeType || ''),
    )
    .sort((left, right) => new Date(left.created).getTime() - new Date(right.created).getTime())
}

export async function storeAssetBuffer({
  assets,
  assetsDir,
  buffer,
  name,
  kind,
  mimeType,
  source,
  sessionId,
  sessionName,
  created,
}) {
  const hash = hashBuffer(buffer)
  const duplicate = await findContentDuplicate(assets, hash)
  if (duplicate) {
    addAssetReference(duplicate, {
      source,
      sessionId,
      sessionName,
      name,
      kind,
      mimeType,
      size: buffer.length,
      created,
    })
    duplicate.modified = created
    if (sessionId && !duplicate.sessionId) duplicate.sessionId = sessionId
    if (sessionName && !duplicate.sessionName) duplicate.sessionName = sessionName
    return duplicate
  }
  const id = randomUUID()
  const storagePath = join(assetsDir, `${id}${extname(name).slice(0, 12)}`)
  await writeFile(storagePath, buffer)
  const asset = {
    id,
    kind,
    name,
    mimeType,
    size: buffer.length,
    hash,
    storagePath,
    source,
    sessionId: sessionId || '',
    sessionName: sessionName || '',
    created,
    modified: created,
  }
  assets.unshift(asset)
  return asset
}

export async function archiveGeneratedAsset({
  assets,
  assetsDir,
  filePath,
  kind,
  mimeType,
  sessionId,
  sessionName,
}) {
  const sourcePath = resolve(filePath)
  const fileInfo = await stat(sourcePath).catch(() => null)
  if (!fileInfo?.isFile()) return null
  const hash = await hashFile(sourcePath)
  const now = new Date().toISOString()
  const duplicate = await findContentDuplicate(assets, hash)
  if (duplicate) {
    addAssetReference(duplicate, {
      source: 'agent',
      sessionId,
      sessionName,
      name: basename(sourcePath),
      kind,
      mimeType,
      size: fileInfo.size,
      filePath: sourcePath,
      created: now,
    })
    duplicate.modified = now
    return duplicate
  }
  const id = randomUUID()
  const name = basename(sourcePath)
  const storagePath = join(assetsDir, `${id}${extname(name).slice(0, 12)}`)
  await copyFile(sourcePath, storagePath)
  const asset = {
    id,
    kind,
    name,
    mimeType,
    size: fileInfo.size,
    hash,
    filePath: sourcePath,
    storagePath,
    source: 'agent',
    sessionId,
    sessionName,
    created: now,
    modified: now,
  }
  assets.unshift(asset)
  return asset
}

export async function reconcileAssetIndex({ assets, assetsDir, save }) {
  const entries = []
  const removed = new Set()
  const cleanupPaths = new Set()
  for (const asset of assets) {
    if (asset.kind === 'link') continue
    const stored = await readablePath(asset.storagePath)
    const source =
      stored ||
      (await firstReadablePath([
        asset.filePath,
        ...assetReferences(asset).map((reference) => reference.filePath),
      ]))
    const storagePath = stored ? managedAssetPath(stored.path, assetsDir) : null
    const hash =
      storagePath && asset.hash
        ? asset.hash
        : source
          ? await hashFile(source.path).catch(() => null)
          : null
    if (!source || !hash) {
      removed.add(asset)
      const cleanupPath = managedAssetPath(asset.storagePath, assetsDir)
      if (cleanupPath) cleanupPaths.add(cleanupPath)
      continue
    }
    entries.push({
      asset,
      hash,
      info: source.info,
      sourcePath: source.path,
      storagePath,
    })
  }

  const groups = new Map()
  for (const entry of entries) {
    const group = groups.get(entry.hash) || []
    group.push(entry)
    groups.set(entry.hash, group)
  }

  let changed = removed.size > 0
  for (const group of groups.values()) {
    const [first] = group
    const canonical = first.asset
    const managedEntry = group.find((entry) => entry.storagePath)
    const readableEntry = group.find((entry) => entry.sourcePath)
    let storagePath = managedEntry?.storagePath || null
    if (!storagePath && readableEntry) {
      const targetPath = join(
        assetsDir,
        `${randomUUID()}${extname(canonical.name || '').slice(0, 12)}`,
      )
      const copied = await copyFile(readableEntry.sourcePath, targetPath)
        .then(() => true)
        .catch(async () => {
          await unlink(targetPath).catch(() => {})
          return false
        })
      if (!copied) continue
      storagePath = targetPath
      changed = true
    }
    if (canonical.hash !== first.hash) {
      canonical.hash = first.hash
      changed = true
    }
    if (storagePath && canonical.storagePath !== storagePath) {
      canonical.storagePath = storagePath
      changed = true
    }
    const size = managedEntry?.info?.size || readableEntry?.info?.size
    if (size != null && canonical.size !== size) {
      canonical.size = size
      changed = true
    }
    for (const duplicate of group.slice(1)) {
      mergeAssetReferences(canonical, duplicate.asset)
      removed.add(duplicate.asset)
      const duplicatePath = managedAssetPath(duplicate.storagePath, assetsDir)
      if (duplicatePath && duplicatePath !== storagePath) cleanupPaths.add(duplicatePath)
      changed = true
    }
  }

  if (!changed) return false
  const remaining = assets.filter((asset) => !removed.has(asset))
  const retainedPaths = new Set(
    remaining.map((asset) => managedAssetPath(asset.storagePath, assetsDir)).filter(Boolean),
  )
  assets.splice(0, assets.length, ...remaining)
  await save()
  for (const filePath of cleanupPaths) {
    if (!retainedPaths.has(filePath)) await unlink(filePath).catch(() => {})
  }
  return true
}
