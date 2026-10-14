import { waitForMobileRuntimeReady } from '@/lib/mobile-runtime-recovery'

const MAX_NATIVE_ASSET_BYTES = 128 * 1024 * 1024

type OpenableAsset = {
  id: string
  name: string
  mimeType?: string
}

function downloadAsset(asset: OpenableAsset) {
  const anchor = document.createElement('a')
  anchor.href = `/api/assets/${encodeURIComponent(asset.id)}/download`
  anchor.download = asset.name
  anchor.hidden = true
  document.body.append(anchor)
  anchor.click()
  anchor.remove()
}

function blobBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onerror = () => reject(reader.error || new Error('读取资产失败。'))
    reader.onload = () => {
      const value = typeof reader.result === 'string' ? reader.result : ''
      const separator = value.indexOf(',')
      if (separator < 0) reject(new Error('资产编码失败。'))
      else resolve(value.slice(separator + 1))
    }
    reader.readAsDataURL(blob)
  })
}

export function canOpenAssetInApplication() {
  if (typeof window === 'undefined') return false
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  return Boolean(window.pisperDesktop?.openAsset || (window.__PISPER_MOBILE_APP__ && invoke))
}

export async function openAssetInApplication(
  asset: OpenableAsset,
): Promise<'application' | 'download'> {
  const desktopOpen = window.pisperDesktop?.openAsset
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!desktopOpen && !(window.__PISPER_MOBILE_APP__ && invoke)) {
    downloadAsset(asset)
    return 'download'
  }

  await waitForMobileRuntimeReady()
  const response = await fetch(`/api/assets/${encodeURIComponent(asset.id)}/download`)
  if (!response.ok) throw new Error(`资产下载失败 (${response.status})`)
  const declaredSize = Number(response.headers.get('content-length')) || 0
  if (declaredSize > MAX_NATIVE_ASSET_BYTES) throw new Error('资产超过 128 MB 原生打开限制。')
  const blob = await response.blob()
  if (!blob.size || blob.size > MAX_NATIVE_ASSET_BYTES)
    throw new Error('资产内容为空或超过 128 MB 原生打开限制。')
  const data = await blobBase64(blob)

  if (desktopOpen) {
    await desktopOpen({ name: asset.name, data })
  } else {
    await invoke?.('mobile_open_asset', {
      input: {
        name: asset.name,
        mimeType: asset.mimeType || blob.type || 'application/octet-stream',
        data,
      },
    })
  }
  return 'application'
}
