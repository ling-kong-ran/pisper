// 本机发布用的 GitHub Releases 接口：只用 Node 自带的 fetch 和一个令牌，三台机器都不必装 gh。
// 令牌只放在请求头里，不写进日志和错误信息。
import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { basename } from 'node:path'
import { Agent } from 'undici'

const API = 'https://api.github.com'
const UPLOADS = 'https://uploads.github.com'
const ATTEMPTS = 4
const REQUEST_TIMEOUT = 15 * 60 * 1000

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

export function createGitHubReleases({
  repository = 'ling-kong-ran/pisper',
  token,
  fetchImpl = fetch,
  sleepImpl = sleep,
  dispatcher = new Agent({
    // 100 MB 以上的组件在慢速上行链路上可能超过 Undici 默认的 300 秒响应头超时。
    headersTimeout: REQUEST_TIMEOUT,
    bodyTimeout: REQUEST_TIMEOUT,
    connectTimeout: 30_000,
  }),
}) {
  if (!token) throw new Error('GITHUB_TOKEN is required for local releases.')
  const headers = (extra = {}) => ({
    Accept: 'application/vnd.github+json',
    Authorization: `Bearer ${token}`,
    'User-Agent': 'pisper-release-local',
    'X-GitHub-Api-Version': '2022-11-28',
    ...extra,
  })

  // GitHub 偶尔回 5xx：重试几次；4xx 是请求本身的问题，直接报出来。
  // GET 可安全重试；其他写请求不盲目重试网络异常，上传由调用方先核对资产。
  async function request(
    method,
    url,
    { body, extraHeaders, raw = false, retryNetwork = method === 'GET', recoverUpload } = {},
  ) {
    for (let attempt = 1; ; attempt += 1) {
      let response
      try {
        response = await fetchImpl(url.startsWith('http') ? url : `${API}${url}`, {
          method,
          headers: headers(extraHeaders),
          body,
          dispatcher,
          redirect: raw ? 'manual' : 'follow',
        })
      } catch (error) {
        if (!retryNetwork) throw error
        const recovered = await recoverUpload?.()
        if (recovered && typeof recovered === 'object') return recovered
        if (attempt >= ATTEMPTS) throw error
        await sleepImpl(2_000 * attempt)
        continue
      }
      // 上一次上传可能恰好在核对后才完成，重复上传会返回 422，再核对一次。
      if (response.status === 422 && recoverUpload) {
        const recovered = await recoverUpload()
        if (recovered && typeof recovered === 'object') return recovered
        if (recovered === true && attempt < ATTEMPTS) {
          await sleepImpl(2_000 * attempt)
          continue
        }
      }
      if (response.status >= 500 && attempt < ATTEMPTS) {
        await sleepImpl(2_000 * attempt)
        continue
      }
      if (raw) return response
      if (response.status === 204) return null
      const text = await response.text()
      if (!response.ok) {
        const message = (() => {
          try {
            return JSON.parse(text).message
          } catch {
            return text.slice(0, 200)
          }
        })()
        throw new Error(
          `GitHub ${method} ${url.split('?')[0]} failed (${response.status}): ${message}`,
        )
      }
      return text ? JSON.parse(text) : null
    }
  }

  const repo = `/repos/${repository}`
  return {
    // 带令牌时草稿也会列出来。
    async listReleases() {
      const releases = []
      for (let page = 1; page <= 10; page += 1) {
        const batch = await request('GET', `${repo}/releases?per_page=100&page=${page}`)
        releases.push(...batch)
        if (batch.length < 100) break
      }
      return releases
    },
    createDraft({ tag, target, title, body }) {
      return request('POST', `${repo}/releases`, {
        body: JSON.stringify({
          tag_name: tag,
          target_commitish: target,
          name: title,
          body,
          draft: true,
          make_latest: 'false',
        }),
      })
    },
    publish(id, { tag, title, body, latest }) {
      return request('PATCH', `${repo}/releases/${id}`, {
        body: JSON.stringify({
          tag_name: tag,
          name: title,
          body,
          draft: false,
          make_latest: latest ? 'true' : 'false',
        }),
      })
    },
    deleteRelease(id) {
      return request('DELETE', `${repo}/releases/${id}`)
    },
    async listAssets(id) {
      const assets = []
      for (let page = 1; page <= 10; page += 1) {
        const batch = await request(
          'GET',
          `${repo}/releases/${id}/assets?per_page=100&page=${page}`,
        )
        assets.push(...batch)
        if (batch.length < 100) break
      }
      return assets
    },
    deleteAsset(assetId) {
      return request('DELETE', `${repo}/releases/assets/${assetId}`)
    },
    // 同名资产先删再传：重跑某个平台的构建时直接覆盖它上一次的产物。
    async uploadAsset(id, path, existing = []) {
      const name = basename(path)
      const previous = existing.find((asset) => asset.name === name)
      if (previous) await this.deleteAsset(previous.id)
      const bytes = await readFile(path)
      const digest = `sha256:${createHash('sha256').update(bytes).digest('hex')}`
      const recoverUpload = async () => {
        const current = (await this.listAssets(id)).find((asset) => asset.name === name)
        if (!current) return false
        // 超时不代表上传失败：完整资产直接复用；半成品先删除，再重新上传。
        if (
          current.state === 'uploaded' &&
          current.size === bytes.length &&
          (!current.digest || current.digest === digest)
        )
          return current
        await this.deleteAsset(current.id)
        return true
      }
      return request(
        'POST',
        `${UPLOADS}${repo}/releases/${id}/assets?name=${encodeURIComponent(name)}`,
        {
          body: bytes,
          retryNetwork: true,
          recoverUpload,
          extraHeaders: {
            'Content-Type': 'application/octet-stream',
            'Content-Length': String(bytes.length),
          },
        },
      )
    },
    // 资产下载会跳到存储服务：跳转后不能再带令牌，手动跟一次。
    async downloadAsset(assetId) {
      const response = await request('GET', `${repo}/releases/assets/${assetId}`, {
        raw: true,
        extraHeaders: { Accept: 'application/octet-stream' },
      })
      if (response.status >= 300 && response.status < 400) {
        const location = response.headers.get('location')
        if (!location) throw new Error(`GitHub asset ${assetId} redirect has no location.`)
        const redirected = await fetchImpl(location, { redirect: 'follow', dispatcher })
        if (!redirected.ok)
          throw new Error(`Asset ${assetId} download failed (${redirected.status}).`)
        return Buffer.from(await redirected.arrayBuffer())
      }
      if (!response.ok) throw new Error(`Asset ${assetId} download failed (${response.status}).`)
      return Buffer.from(await response.arrayBuffer())
    },
  }
}
