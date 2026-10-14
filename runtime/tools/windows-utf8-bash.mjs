import { createBashTool } from '../runtime/pi-coding-agent.mjs'

// Windows UTF-8 环境修正：保证 Windows 上子进程输出按 UTF-8 编码，
// 避免中文/emoji 在 GBK 默认环境下乱码。
export const WINDOWS_UTF8_ENV = Object.freeze({
  PYTHONIOENCODING: 'utf-8',
  PYTHONUTF8: '1',
  LANG: 'C.UTF-8',
  LC_ALL: 'C.UTF-8',
})

export function applyWindowsUtf8Environment(context, platform = process.platform) {
  if (platform !== 'win32') return context
  return {
    ...context,
    env: {
      ...context.env,
      ...WINDOWS_UTF8_ENV,
    },
  }
}

export function createWindowsUtf8BashTool(cwd, platform = process.platform) {
  if (platform !== 'win32') return null
  return createBashTool(cwd, {
    spawnHook: (context) => applyWindowsUtf8Environment(context, platform),
  })
}
