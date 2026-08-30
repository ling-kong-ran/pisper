import { createReadStream } from 'node:fs'

// 记忆与资产路由：目录浏览、资产（上传/下载/删除）、记忆空间/候选/搜索、
// 会话树标签等辅助能力。

function assetByteRange(header, size) {
  const match = /^bytes=(\d*)-(\d*)$/.exec(String(header || '').trim())
  if (!match) return null
  let start = match[1] ? Number(match[1]) : null
  let end = match[2] ? Number(match[2]) : null
  if (start === null && end !== null) {
    start = Math.max(0, size - end)
    end = size - 1
  } else {
    start ??= 0
    end = Math.min(end ?? size - 1, size - 1)
  }
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start < 0 || start > end)
    return null
  return { start, end }
}
export const memoryAssetRoutes = [
  {
    method: 'GET',
    path: '/api/directories',
    async handler({ runtime, url, json }) {
      json(200, await runtime.listDirectories(url.searchParams.get('path')))
    },
  },
  {
    method: 'GET',
    path: '/api/workspace-entries',
    async handler({ runtime, url, json }) {
      json(200, await runtime.listWorkspaceEntries(url.searchParams.get('path')))
    },
  },
  {
    method: 'GET',
    path: '/api/assets',
    async handler({ runtime, url, json }) {
      json(200, {
        assets: await runtime.listAssets({
          query: url.searchParams.get('query'),
          kind: url.searchParams.get('kind'),
          sessionId: url.searchParams.get('sessionId'),
        }),
      })
    },
  },
  {
    method: 'POST',
    path: '/api/assets',
    async handler({ runtime, body, json }) {
      json(201, await runtime.createAsset(await body()))
    },
  },
  {
    method: 'GET',
    path: '/api/assets/:assetId/content',
    async handler({ runtime, params, url, json }) {
      const content = await runtime.getAssetContent(params.assetId, {
        previewOnly: url.searchParams.get('preview') === '1',
      })
      if (!content) json(404, { error: '资产不存在。' })
      else json(200, content)
    },
  },
  {
    method: 'GET',
    path: '/api/assets/:assetId/download',
    async handler({ runtime, params, url, req, res, json }) {
      const download = await runtime.getAssetDownload(params.assetId, { includeBuffer: false })
      if (!download) {
        json(404, { error: '资产不存在或不可下载。' })
        return
      }
      const rangeHeader = req.headers.range
      const range = rangeHeader ? assetByteRange(rangeHeader, download.size) : null
      if (rangeHeader && !range) {
        res.writeHead(416, { 'Content-Range': `bytes */${download.size}` })
        res.end()
        return
      }
      const start = range?.start ?? 0
      const end = range?.end ?? Math.max(0, download.size - 1)
      res.writeHead(range ? 206 : 200, {
        'Content-Type': download.asset.mimeType || 'application/octet-stream',
        'Content-Length': Math.max(0, end - start + 1),
        'Content-Disposition': `${url.searchParams.get('inline') === '1' ? 'inline' : 'attachment'}; filename*=UTF-8''${encodeURIComponent(download.asset.name)}`,
        'Accept-Ranges': 'bytes',
        ...(range ? { 'Content-Range': `bytes ${start}-${end}/${download.size}` } : {}),
        'Cache-Control': 'private, max-age=60',
      })
      createReadStream(download.path, range ? { start, end } : undefined).pipe(res)
    },
  },
  {
    method: 'DELETE',
    path: '/api/assets/:assetId',
    async handler({ runtime, params, json }) {
      const deleted = await runtime.deleteAsset(params.assetId)
      if (!deleted) json(404, { error: '资产不存在。' })
      else json(200, { deleted: true })
    },
  },
  {
    method: 'GET',
    path: '/api/memory',
    handler({ runtime, url, json }) {
      json(
        200,
        runtime.getMemoryDashboard({
          query: url.searchParams.get('query') || '',
          spaceId: url.searchParams.get('spaceId') || '',
        }),
      )
    },
  },
  {
    method: 'GET',
    path: '/api/memory/candidates',
    handler({ runtime, url, json }) {
      json(200, runtime.getMemoryCandidateInbox({ limit: url.searchParams.get('limit') }))
    },
  },
  {
    method: 'POST',
    path: '/api/memory/spaces',
    async handler({ runtime, body, json }) {
      json(201, runtime.createMemorySpace(await body()))
    },
  },
  {
    method: 'PATCH',
    path: '/api/memory/spaces/:spaceId',
    async handler({ runtime, params, body, json }) {
      const updated = runtime.updateMemorySpace(params.spaceId, await body())
      if (!updated) json(404, { error: '星域不存在。' })
      else json(200, updated)
    },
  },
  {
    method: 'DELETE',
    path: '/api/memory/spaces/:spaceId',
    handler({ runtime, params, json }) {
      const deleted = runtime.deleteMemorySpace(params.spaceId)
      if (!deleted) json(404, { error: '星域不存在。' })
      else json(200, { deleted: true })
    },
  },
  {
    method: 'POST',
    path: '/api/memory/nodes',
    async handler({ runtime, body, json }) {
      json(201, runtime.createMemory(await body()))
    },
  },
  {
    method: 'PATCH',
    path: '/api/memory/nodes/:memoryId',
    async handler({ runtime, params, body, json }) {
      const updated = runtime.updateMemory(params.memoryId, await body())
      if (!updated) json(404, { error: '星辰不存在。' })
      else json(200, updated)
    },
  },
  {
    method: 'DELETE',
    path: '/api/memory/nodes/:memoryId',
    handler({ runtime, params, json }) {
      const deleted = runtime.deleteMemory(params.memoryId)
      if (!deleted) json(404, { error: '星辰不存在。' })
      else json(200, { deleted: true })
    },
  },
  {
    method: 'POST',
    path: '/api/memory/candidates/reject-all',
    handler({ runtime, json }) {
      json(200, runtime.rejectAllMemoryCandidates())
    },
  },
  {
    method: 'POST',
    path: '/api/memory/candidates/:candidateId/:action',
    where: { action: ['accept', 'reject'] },
    handler({ runtime, params, json }) {
      const result =
        params.action === 'accept'
          ? runtime.acceptMemoryCandidate(params.candidateId)
          : runtime.rejectMemoryCandidate(params.candidateId)
      if (!result) json(404, { error: '候选记忆不存在或已处理。' })
      else json(200, result)
    },
  },
]
