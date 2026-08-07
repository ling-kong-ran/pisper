import { copyFile, mkdir, readFile, rename, unlink, writeFile } from 'node:fs/promises'
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

export async function writeJsonAtomic(path, value) {
  await mkdir(dirname(path), { recursive: true })
  const temporary = `${path}.${process.pid}.${Date.now()}.tmp`
  await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, 'utf8')

  let lastError
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      await rename(temporary, path)
      return
    } catch (error) {
      lastError = error
      if (!isReplaceConflict(error)) break
      try {
        // Windows cannot reliably rename over an existing path; replace via copy.
        await copyFile(temporary, path)
        await unlink(temporary).catch(() => {})
        return
      } catch (replaceError) {
        lastError = replaceError
        if (!isReplaceConflict(replaceError) && replaceError?.code !== 'ENOENT') break
        await sleep(20 * (attempt + 1))
      }
    }
  }

  await unlink(temporary).catch(() => {})
  throw lastError
}
