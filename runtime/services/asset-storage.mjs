import { copyFile, stat } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { basename, extname, join, resolve } from 'node:path'

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
  const now = new Date().toISOString()
  const existing = assets.find(
    (asset) => asset.source === 'agent' && asset.filePath === sourcePath && asset.storagePath,
  )
  if (existing) {
    await copyFile(sourcePath, existing.storagePath)
    Object.assign(existing, { size: fileInfo.size, modified: now, sessionId, sessionName })
    return existing
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
