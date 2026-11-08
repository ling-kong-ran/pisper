// 本地路径定位服务：由 Runtime 在宿主系统中打开文件管理器，不依赖桌面壳桥接。
import { spawn } from 'node:child_process'
import { stat } from 'node:fs/promises'
import { dirname, isAbsolute, parse, resolve } from 'node:path'
import { pisperError } from '../../shared/errors.mjs'

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
    throw pisperError('invalid', 'path')
  }
  if (!isAbsolute(path)) throw pisperError('invalid', 'path', 'must be absolute')
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
  throw pisperError('not_found', 'path')
}

function commandFor(target, platform = process.platform) {
  if (platform === 'win32') {
    // 部分 Windows Explorer 版本会静默忽略正斜杠路径，导致请求成功但窗口不出现。
    const windowsPath = target.path.replaceAll('/', '\\')
    // /select 参数必须原样交给 Explorer：Node 默认会把内嵌引号转义成 \"，Explorer 解析失败后
    // 退回打开“文档”等默认目录。Windows 路径不可能含双引号，原样传递不会被拆开。
    return target.isDirectory
      ? { command: 'explorer.exe', args: [windowsPath] }
      : { command: 'explorer.exe', args: [`/select,"${windowsPath}"`], verbatim: true }
  }
  if (platform === 'darwin') {
    return target.isDirectory
      ? { command: 'open', args: [target.path] }
      : { command: 'open', args: ['-R', target.path] }
  }
  // xdg-open 打开文件会用默认程序打开它本身；定位文件时打开所在目录。
  return { command: 'xdg-open', args: [target.isDirectory ? target.path : dirname(target.path)] }
}

function launch(command, args, verbatim = false) {
  return new Promise((resolvePromise, reject) => {
    let settled = false
    const child = spawn(command, args, {
      detached: process.platform !== 'win32',
      stdio: 'ignore',
      windowsHide: process.platform !== 'win32',
      windowsVerbatimArguments: verbatim,
    })
    const timer = setTimeout(() => {
      if (settled) return
      settled = true
      child.kill()
      reject(pisperError('timeout', 'operation', `file manager ${command}`))
    }, SPAWN_TIMEOUT_MS)
    child.once('error', (error) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      reject(pisperError('failed', 'operation', `file manager: ${error.message}`))
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
        else
          reject(
            pisperError(
              'failed',
              'operation',
              `file manager ${command} exited with ${code ?? 'unknown'}`,
            ),
          )
      })
    }
  })
}

export async function revealLocalPath(value) {
  const requestedPath = validatePath(value)
  const target = await existingTarget(requestedPath)
  const { command, args, verbatim } = commandFor(target)
  try {
    await launch(command, args, verbatim)
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
