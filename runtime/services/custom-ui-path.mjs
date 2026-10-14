import { homedir } from 'node:os'
import { posix, win32 } from 'node:path'

/**
 * 目录来自 Runtime 主机，不能按浏览器所在设备选择分隔符或主目录缩写。
 * Windows 使用资源管理器可展开的变量，避免把用户名展示到页面上。
 * @param {string} path
 * @param {{ home?: string, platform?: NodeJS.Platform }} [options]
 * @returns {string}
 */
export function displayCustomUiPath(path, { home = homedir(), platform = process.platform } = {}) {
  const paths = platform === 'win32' ? win32 : posix
  if (!home || !paths.isAbsolute(home) || !paths.isAbsolute(path)) return path

  const relative = paths.relative(home, path)
  if (relative === '..' || relative.startsWith(`..${paths.sep}`) || paths.isAbsolute(relative)) {
    return path
  }

  const homeLabel = platform === 'win32' ? '%USERPROFILE%' : '~'
  return relative ? `${homeLabel}${paths.sep}${relative}` : homeLabel
}
