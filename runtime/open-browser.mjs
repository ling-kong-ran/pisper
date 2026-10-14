// 启动时自动打开浏览器的辅助模块：统一处理平台差异（Windows/macOS/Linux 的拉起命令不同）。
import { spawn as spawnProcess } from 'node:child_process'

// 只有本地回环地址才允许自动打开浏览器，防止在远程/服务器环境下把浏览器开到远端机器上。
const LOCAL_HOSTS = new Set(['127.0.0.1', 'localhost', '::1'])

// 判定条件：显式设置 PISPER_OPEN_BROWSER=1 且非 CI 环境且 host 是本地地址。
export function shouldOpenBrowser({ host, env = process.env } = {}) {
  if (String(env.PISPER_OPEN_BROWSER || '').trim() !== '1') return false
  if (String(env.CI || '').trim()) return false
  return LOCAL_HOSTS.has(
    String(host || '')
      .trim()
      .toLowerCase(),
  )
}

// 返回平台对应的浏览器拉起命令；非 HTTP(S) URL 一律拒绝，避免协议注入。
export function browserLaunchSpec(url, platform = process.platform) {
  const target = new URL(String(url || ''))
  if (!['http:', 'https:'].includes(target.protocol))
    throw new Error('Only HTTP/HTTPS URLs can be opened in the browser.')
  if (platform === 'win32')
    return { command: 'rundll32.exe', args: ['url.dll,FileProtocolHandler', target.href] }
  if (platform === 'darwin') return { command: 'open', args: [target.href] }
  return { command: 'xdg-open', args: [target.href] }
}

// 以 detached + unref 方式拉起浏览器：不阻塞当前进程，也不等待子进程退出。
// 拉不起时静默返回 false（例如无图形环境），由调用方决定是否提示。
export function openBrowser(url, { platform = process.platform, spawn = spawnProcess } = {}) {
  try {
    const { command, args } = browserLaunchSpec(url, platform)
    const child = spawn(command, args, {
      detached: true,
      stdio: 'ignore',
      windowsHide: true,
    })
    child.once?.('error', () => {})
    child.unref?.()
    return true
  } catch {
    return false
  }
}
