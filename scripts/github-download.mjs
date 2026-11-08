// 构建机从 GitHub Releases 下载固定资源（搜索工具、Android 嵌入式 Node、桌面内置组件）共用的下载。
// github.com 的 releases/download 跳转入口偶尔整段时间回 504（2026-11-08 连续几次发版都卡在这里），
// 而 GitHub API 照常可用：每一轮先走原地址，暂时性失败（断连、5xx）时按仓库、标签、文件名经 API
// 下载同一个文件；两条路都失败才按 5、10、20、40 秒退避，共 5 轮。4xx、超出大小与调用方校验失败
// （摘要、签名）不重试。调用方照旧校验内容，换的只是下载路径。
export class TransientDownloadError extends Error {}

const GITHUB_RELEASE_ASSET =
  /^https:\/\/github\.com\/([^/]+)\/([^/]+)\/releases\/download\/([^/]+)\/([^/]+)$/

export function githubHeaders(env = process.env) {
  const token = env.GH_TOKEN || env.GITHUB_TOKEN
  return {
    'User-Agent': 'pisper-release',
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
  }
}

// 不是 GitHub Releases 地址时返回空字符串（没有备用路径）。
export async function githubApiAssetUrl(url, fetchImpl = globalThis.fetch, env = process.env) {
  const match = GITHUB_RELEASE_ASSET.exec(url)
  if (!match) return ''
  const [, owner, repo, tag, name] = match
  let response
  try {
    response = await fetchImpl(
      `https://api.github.com/repos/${owner}/${repo}/releases/tags/${tag}`,
      {
        headers: { Accept: 'application/vnd.github+json', ...githubHeaders(env) },
        signal: AbortSignal.timeout(30_000),
      },
    )
  } catch (error) {
    throw new TransientDownloadError(`release lookup failed: ${error?.message || error}`)
  }
  if (!response.ok)
    throw new TransientDownloadError(`release lookup failed with HTTP ${response.status}`)
  const release = await response.json()
  const id = (Array.isArray(release?.assets) ? release.assets : []).find(
    (item) => item?.name === decodeURIComponent(name),
  )?.id
  if (!Number.isSafeInteger(id)) throw new TransientDownloadError('release asset not found')
  return `https://api.github.com/repos/${owner}/${repo}/releases/assets/${id}`
}

async function fetchBytes(url, { fetchImpl, headers, timeoutMs, maxBytes }) {
  let response
  try {
    response = await fetchImpl(url, {
      headers,
      redirect: 'follow',
      signal: AbortSignal.timeout(timeoutMs),
    })
  } catch (error) {
    throw new TransientDownloadError(`download failed: ${error?.message || error}: ${url}`)
  }
  if (response.status >= 500)
    throw new TransientDownloadError(`download failed with HTTP ${response.status}: ${url}`)
  if (!response.ok || !response.body)
    throw new Error(`download failed with HTTP ${response.status}: ${url}`)
  if (Number(response.headers.get('content-length') || 0) > maxBytes)
    throw new Error(`download is larger than ${maxBytes} bytes: ${url}`)
  const chunks = []
  let size = 0
  try {
    for await (const chunk of response.body) {
      size += chunk.length
      if (size > maxBytes) break
      chunks.push(chunk)
    }
  } catch (error) {
    throw new TransientDownloadError(`download interrupted: ${error?.message || error}: ${url}`)
  }
  if (size > maxBytes) throw new Error(`download is larger than ${maxBytes} bytes: ${url}`)
  return Buffer.concat(chunks)
}

/**
 * @param {string} url
 * @param {{ fetchImpl?: typeof fetch, env?: NodeJS.ProcessEnv, attempts?: number,
 *   retryDelayMs?: number, timeoutMs?: number, maxBytes?: number,
 *   headers?: Record<string, string>, verify?: (bytes: Buffer) => void }} [options]
 */
export async function downloadReleaseAsset(
  url,
  {
    fetchImpl = globalThis.fetch,
    env = process.env,
    attempts = 5,
    retryDelayMs = 5_000,
    timeoutMs = 120_000,
    maxBytes = Number.POSITIVE_INFINITY,
    headers = {},
    verify = () => {},
  } = {},
) {
  const options = { fetchImpl, timeoutMs, maxBytes }
  const sources = [
    () => fetchBytes(url, { ...options, headers: { 'User-Agent': 'pisper-release', ...headers } }),
    async () => {
      const apiUrl = await githubApiAssetUrl(url, fetchImpl, env)
      if (!apiUrl) throw new TransientDownloadError(`no fallback source: ${url}`)
      return fetchBytes(apiUrl, {
        ...options,
        headers: { ...headers, ...githubHeaders(env), Accept: 'application/octet-stream' },
      })
    },
  ]
  for (let attempt = 1; ; attempt += 1) {
    let failure
    for (const source of sources) {
      try {
        const bytes = await source()
        verify(bytes)
        return bytes
      } catch (error) {
        if (!(error instanceof TransientDownloadError)) throw error
        failure ||= error
      }
    }
    if (attempt >= attempts) throw failure
    await new Promise((resolve) => setTimeout(resolve, retryDelayMs * 2 ** (attempt - 1)))
  }
}
