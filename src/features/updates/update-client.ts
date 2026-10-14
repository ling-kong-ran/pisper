// 更新客户端：桌面桥接的更新操作封装，复用共享的发布地址常量。
import type { UpdateStatus } from '@/types/update'

export { RELEASES_URL } from '@shared/app-update.mjs'

export async function checkWebUpdates({
  refresh = false,
  fetcher = fetch,
}: { refresh?: boolean; fetcher?: typeof fetch } = {}): Promise<UpdateStatus> {
  const response = await fetcher(`/api/app-update${refresh ? '?refresh=1' : ''}`, {
    cache: 'no-store',
  })
  const payload = (await response.json().catch(() => null)) as
    UpdateStatus | { error?: unknown } | null
  if (!response.ok) {
    const detail =
      payload && 'error' in payload && typeof payload.error === 'string' ? payload.error.trim() : ''
    throw new Error(detail || `更新检查失败：HTTP ${response.status}`)
  }
  return payload as UpdateStatus
}
