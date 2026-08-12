import { chmod, copyFile, mkdir, readFile, rename, unlink, writeFile } from 'node:fs/promises'
import { dirname } from 'node:path'

export async function readJson(path, fallback) {
  try {
    return JSON.parse(await readFile(path, 'utf8'))
  } catch (error) {
    if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return fallback
    throw error
  }
}

function isReplaceConflict(error) {
  return (
    error?.code === 'EPERM' ||
    error?.code === 'EEXIST' ||
    error?.code === 'EACCES' ||
    error?.code === 'EBUSY'
  )
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

export async function writeJsonAtomic(path, value, { mode } = {}) {
  await mkdir(dirname(path), { recursive: true })
  const temporary = `${path}.${process.pid}.${Date.now()}.tmp`
  const posixMode = process.platform === 'win32' ? undefined : mode
  await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: 'utf8',
    ...(posixMode === undefined ? {} : { mode: posixMode }),
  })

  let lastError
  for (let attempt = 0; attempt < 5; attempt += 1) {
    let replaced = false
    try {
      await rename(temporary, path)
      replaced = true
    } catch (error) {
      lastError = error
      if (!isReplaceConflict(error)) break
      try {
        // Windows cannot reliably rename over an existing path; replace via copy.
        await copyFile(temporary, path)
        await unlink(temporary).catch(() => {})
        replaced = true
      } catch (replaceError) {
        lastError = replaceError
        if (!isReplaceConflict(replaceError) && replaceError?.code !== 'ENOENT') break
        await sleep(20 * (attempt + 1))
      }
    }
    if (replaced) {
      if (posixMode !== undefined) await chmod(path, posixMode)
      return
    }
  }

  await unlink(temporary).catch(() => {})
  throw lastError
}
