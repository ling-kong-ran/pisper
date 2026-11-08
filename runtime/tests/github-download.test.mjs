// 构建机从 GitHub Releases 下载固定资源：github.com 入口回 5xx 时经 GitHub API 下同一个文件（带令牌），
// 不是 GitHub Releases 的地址没有备用路径；调用方校验失败、4xx 都不重试。
import assert from 'node:assert/strict'
import test from 'node:test'
import { downloadReleaseAsset, githubApiAssetUrl } from '../../scripts/github-download.mjs'

const ASSET = 'https://github.com/owner/repo/releases/download/v1.2.3/tool-linux.tar.gz'

test('a 504 from github.com falls back to the API download with the CI token', async () => {
  const calls = []
  const bytes = await downloadReleaseAsset(ASSET, {
    env: { GH_TOKEN: 'ci-token' },
    retryDelayMs: 1,
    fetchImpl: async (url, options) => {
      calls.push([url, options?.headers?.Authorization || '', options?.headers?.Accept || ''])
      if (url === ASSET) return new Response('busy', { status: 504 })
      if (url.endsWith('/releases/tags/v1.2.3'))
        return Response.json({ assets: [{ id: 7, name: 'tool-linux.tar.gz' }] })
      return new Response('payload')
    },
  })
  assert.equal(bytes.toString(), 'payload')
  assert.deepEqual(calls, [
    [ASSET, '', ''],
    [
      'https://api.github.com/repos/owner/repo/releases/tags/v1.2.3',
      'Bearer ci-token',
      'application/vnd.github+json',
    ],
    [
      'https://api.github.com/repos/owner/repo/releases/assets/7',
      'Bearer ci-token',
      'application/octet-stream',
    ],
  ])
})

test('verification failures, client errors and oversize downloads are not retried', async () => {
  let calls = 0
  const fetchImpl = async () => {
    calls += 1
    return new Response('payload')
  }
  await assert.rejects(
    downloadReleaseAsset(ASSET, {
      fetchImpl,
      retryDelayMs: 1,
      verify: () => {
        throw new Error('checksum mismatch')
      },
    }),
    /checksum mismatch/,
  )
  assert.equal(calls, 1)
  await assert.rejects(
    downloadReleaseAsset(ASSET, { fetchImpl, retryDelayMs: 1, maxBytes: 3 }),
    /larger than 3 bytes/,
  )
  calls = 0
  await assert.rejects(
    downloadReleaseAsset(ASSET, {
      retryDelayMs: 1,
      fetchImpl: async () => {
        calls += 1
        return new Response('gone', { status: 404 })
      },
    }),
    /HTTP 404/,
  )
  assert.equal(calls, 1)
})

test('other hosts have no fallback and retry on their own up to the limit', async () => {
  assert.equal(await githubApiAssetUrl('https://example.test/tool.tar.gz'), '')
  let calls = 0
  await assert.rejects(
    downloadReleaseAsset('https://example.test/tool.tar.gz', {
      retryDelayMs: 1,
      attempts: 3,
      fetchImpl: async () => {
        calls += 1
        return new Response('busy', { status: 503 })
      },
    }),
    /HTTP 503/,
  )
  assert.equal(calls, 3)
})
