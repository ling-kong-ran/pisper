// JSON 文件读写工具：读时容错（文件不存在返回 fallback），
// 写时原子替换（临时文件 + rename）并保留可选文件权限（如 0600 密钥文件）。
import { chmod, copyFile, mkdir, readFile, rename, unlink, writeFile } from 'node:fs/promises'
import { dirname } from 'node:path'

// 读取 JSON；文件不存在/非目录时返回 fallback，其他错误上抛。
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

// 原子写 JSON：先写临时文件再 rename 替换；Windows 上 rename 冲突时短暂重试。
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
