// 本地路径定位服务：由 Runtime 在宿主系统中打开文件管理器，不依赖桌面壳桥接。
import { spawn } from 'node:child_process'
import { stat } from 'node:fs/promises'
import { dirname, isAbsolute, parse, resolve } from 'node:path'

const MAX_PATH_LENGTH = 32_768
const MAX_ANCESTOR_HOPS = 32
const SPAWN_TIMEOUT_MS = 8_000

function validatePath(value) {
  const path = String(value || '').trim()
  if (
    !path ||
    path.length > MAX_PATH_LENGTH ||
    [...path].some((character) => {
      const codePoint = character.codePointAt(0) || 0
      return codePoint < 32 || codePoint === 127
    })
  ) {
    throw new Error('本地路径无效。')
  }
  if (!isAbsolute(path)) throw new Error('本地路径必须是绝对路径。')
  return resolve(path)
}

async function existingTarget(path) {
  try {
    const info = await stat(path)
    return { path, isDirectory: info.isDirectory() }
  } catch {
    let current = path
    for (let index = 0; index < MAX_ANCESTOR_HOPS; index += 1) {
      const parent = dirname(current)
      if (parent === current || parent === parse(current).root) break
      try {
        const info = await stat(parent)
        if (info.isDirectory()) return { path: parent, isDirectory: true, fallback: true }
      } catch {}
      current = parent
    }
  }
  throw new Error(`本地路径不存在：${path}`)
}

function commandFor(target, platform = process.platform) {
  if (platform === 'win32') {
    // 部分 Windows Explorer 版本会静默忽略正斜杠路径，导致请求成功但窗口不出现。
    const windowsPath = target.path.replaceAll('/', '\\')
    return target.isDirectory
      ? { command: 'explorer.exe', args: [windowsPath] }
      : { command: 'explorer.exe', args: [`/select,"${windowsPath}"`] }
  }
  if (platform === 'darwin') {
    return target.isDirectory
      ? { command: 'open', args: [target.path] }
      : { command: 'open', args: ['-R', target.path] }
  }
  return { command: 'xdg-open', args: [target.path] }
}

function launch(command, args) {
  return new Promise((resolvePromise, reject) => {
    let settled = false
    const child = spawn(command, args, {
      detached: process.platform !== 'win32',
      stdio: 'ignore',
      windowsHide: process.platform !== 'win32',
    })
    const timer = setTimeout(() => {
      if (settled) return
      settled = true
      child.kill()
      reject(new Error(`启动文件管理器超时：${command}`))
    }, SPAWN_TIMEOUT_MS)
    child.once('error', (error) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      reject(new Error(`启动文件管理器失败：${error.message}`))
    })
    child.once('spawn', () => {
      if (process.platform === 'win32') {
        if (settled) return
        settled = true
        clearTimeout(timer)
        child.unref()
        resolvePromise(true)
      }
    })
    if (process.platform !== 'win32') {
      // macOS/Linux 的 open/xdg-open 会在交给桌面环境后退出；检查退出码才能识别启动失败。
      child.once('close', (code) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        if (code === 0) resolvePromise(true)
        else reject(new Error(`文件管理器退出失败：${command}（退出码 ${code ?? 'unknown'}）`))
      })
    }
  })
}

export async function revealLocalPath(value) {
  const requestedPath = validatePath(value)
  const target = await existingTarget(requestedPath)
  const { command, args } = commandFor(target)
  try {
    await launch(command, args)
    return { revealed: true, path: target.path, fallback: Boolean(target.fallback) }
  } catch (error) {
    if (target.isDirectory) throw error
    const parent = dirname(target.path)
    const fallback = commandFor({ path: parent, isDirectory: true })
    await launch(fallback.command, fallback.args)
    return { revealed: true, path: parent, fallback: true }
  }
}

export function revealPathCommandForTests(value, platform = process.platform, isDirectory = false) {
  return commandFor({ path: value, isDirectory }, platform)
}
