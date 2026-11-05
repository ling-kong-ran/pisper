import { createReadStream } from 'node:fs'

// 记忆与资产路由：资产（上传/下载/删除）、记忆空间/候选/搜索、
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
// 外部输入先归一化：状态只认这几个，时间只认能解析的。
function memoryStatusParam(value) {
  return ['current', 'archived', 'history', 'all'].includes(value) ? value : 'current'
}

function isoParam(value) {
  const time = Date.parse(String(value || ''))
  return Number.isNaN(time) ? '' : new Date(time).toISOString()
}

export const memoryAssetRoutes = [
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
          status: memoryStatusParam(url.searchParams.get('status')),
          before: isoParam(url.searchParams.get('before')),
          limit: url.searchParams.get('limit') || undefined,
        }),
      )
    },
  },
  {
    method: 'GET',
    path: '/api/memory/evolution',
    handler({ runtime, url, json }) {
      json(200, runtime.getMemoryEvolution({ spaceId: url.searchParams.get('spaceId') || '' }))
    },
  },
  {
    method: 'GET',
    path: '/api/memory/events',
    handler({ runtime, url, json }) {
      json(200, {
        events: runtime.listMemoryEvents({
          spaceId: url.searchParams.get('spaceId') || '',
          before: isoParam(url.searchParams.get('before')),
          limit: url.searchParams.get('limit') || undefined,
        }),
      })
    },
  },
  {
    method: 'POST',
    path: '/api/memory/events/:eventId/undo',
    handler({ runtime, params, json }) {
      const result = runtime.undoMemoryEvent(params.eventId)
      if (!result) json(404, { error: '这次改动已经不能撤销。' })
      else json(200, result)
    },
  },
  {
    method: 'POST',
    path: '/api/memory/consolidate',
    async handler({ runtime, json }) {
      json(200, await runtime.consolidateMemory())
    },
  },
  {
    method: 'POST',
    path: '/api/memory/episodes/clear',
    async handler({ runtime, body, json }) {
      const input = await body()
      json(200, runtime.clearMemoryEpisodes({ spaceId: String(input?.spaceId || '') }))
    },
  },
  {
    method: 'GET',
    path: '/api/memory/export',
    handler({ runtime, url, json }) {
      json(200, {
        markdown: runtime.exportMemory({ spaceId: url.searchParams.get('spaceId') || '' }),
      })
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
      if (!updated) json(404, { error: '记忆空间不存在。' })
      else json(200, updated)
    },
  },
  {
    method: 'DELETE',
    path: '/api/memory/spaces/:spaceId',
    handler({ runtime, params, json }) {
      const deleted = runtime.deleteMemorySpace(params.spaceId)
      if (!deleted) json(404, { error: '记忆空间不存在。' })
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
      if (!updated) json(404, { error: '记忆不存在。' })
      else json(200, updated)
    },
  },
  {
    method: 'DELETE',
    path: '/api/memory/nodes/:memoryId',
    handler({ runtime, params, json }) {
      const deleted = runtime.deleteMemory(params.memoryId)
      if (!deleted) json(404, { error: '记忆不存在。' })
      else json(200, { deleted: true })
    },
  },
  {
    method: 'GET',
    path: '/api/memory/nodes/:memoryId',
    handler({ runtime, params, json }) {
      const detail = runtime.getMemoryDetail(params.memoryId)
      if (!detail) json(404, { error: '记忆不存在。' })
      else json(200, detail)
    },
  },
  {
    method: 'POST',
    path: '/api/memory/nodes/:memoryId/:action',
    where: { action: ['restore', 'confirm', 'pin', 'unpin'] },
    handler({ runtime, params, json }) {
      const result =
        params.action === 'restore'
          ? runtime.restoreMemory(params.memoryId)
          : params.action === 'confirm'
            ? runtime.confirmMemory(params.memoryId)
            : runtime.setMemoryPinned(params.memoryId, params.action === 'pin')
      if (!result) json(404, { error: '记忆不存在。' })
      else json(200, result)
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
