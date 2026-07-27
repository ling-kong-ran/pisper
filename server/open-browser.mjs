import { spawn as spawnProcess } from 'node:child_process'

const LOCAL_HOSTS = new Set(['127.0.0.1', 'localhost', '::1'])

export function shouldOpenBrowser({ host, env = process.env } = {}) {
  if (String(env.PISPER_OPEN_BROWSER || '').trim() !== '1') return false
  if (String(env.CI || '').trim()) return false
  return LOCAL_HOSTS.has(String(host || '').trim().toLowerCase())
}

export function browserLaunchSpec(url, platform = process.platform) {
  const target = new URL(String(url || ''))
  if (!['http:', 'https:'].includes(target.protocol)) throw new Error('Only HTTP/HTTPS URLs can be opened in the browser.')
  if (platform === 'win32') return { command: 'rundll32.exe', args: ['url.dll,FileProtocolHandler', target.href] }
  if (platform === 'darwin') return { command: 'open', args: [target.href] }
  return { command: 'xdg-open', args: [target.href] }
}

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
