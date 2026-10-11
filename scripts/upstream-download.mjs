import { createHash } from 'node:crypto'

export const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex')

// 发布输入有总时限和大小边界；错误正文不进入日志，防止代理服务回显凭据。
export async function download(
  url,
  { fetchImpl = fetch, maxBytes = 16 * 1024 * 1024, timeoutMs = 30000 } = {},
) {
  const response = await fetchImpl(url, {
    signal: AbortSignal.timeout(timeoutMs),
    redirect: 'error',
    headers: { 'User-Agent': 'Pisper-release-updater', Accept: 'application/json' },
  })
  if (!response.ok) throw new Error(`Upstream download failed: HTTP ${response.status}`)
  if (!response.body) throw new Error('Upstream response has no body')
  const reader = response.body.getReader()
  const chunks = []
  let size = 0
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      size += value.length
      if (size > maxBytes) throw new Error('Upstream response exceeds size limit')
      chunks.push(value)
    }
  } catch (error) {
    await reader.cancel().catch(() => {})
    throw error
  } finally {
    reader.releaseLock()
  }
  return { bytes: Buffer.concat(chunks), headers: response.headers }
}
