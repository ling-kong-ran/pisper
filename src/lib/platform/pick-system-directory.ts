import { apiJson } from '@/lib/http/api'
import { invalidResponseError } from '@/lib/http/http-response'
import { translateText } from '@/app/i18n/i18n'
import { importMobileWorkspaceDirectory } from '@/lib/mobile/mobile-workspace'

let picking = false

export function hasSystemDirectoryPicker(): boolean {
  return typeof window !== 'undefined' && window.__PISPER_MOBILE_PLATFORM__ !== 'ios'
}

// 系统选择器是唯一浏览入口；浏览器不能提供绝对目录，交由本机 Runtime 打开。
export async function pickSystemDirectory(initialDirectory = ''): Promise<string | null> {
  if (picking) return null
  picking = true
  try {
    if (window.__PISPER_MOBILE_APP__) {
      if (window.__PISPER_MOBILE_PLATFORM__ !== 'android') {
        throw new Error(translateText('common:workspacePicker.systemUnavailable'))
      }
      return await importMobileWorkspaceDirectory()
    }
    if (window.pisperDesktop?.pickDirectory) {
      return await window.pisperDesktop.pickDirectory(initialDirectory || undefined)
    }
    const result = await apiJson<unknown>('/api/desktop/pick-directory', {
      method: 'POST',
      body: JSON.stringify({ initialDirectory }),
      timeout: 610_000,
    })
    if (!result || typeof result !== 'object' || !('path' in result)) throw invalidResponseError()
    const path = result.path
    if (path !== null && (typeof path !== 'string' || !path.trim())) throw invalidResponseError()
    return path
  } finally {
    picking = false
  }
}
