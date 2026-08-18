// 记忆与资产路由：目录浏览、资产（上传/下载/删除）、记忆空间/候选/搜索、
// 会话树标签等辅助能力。
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
    async handler({ runtime, params, json }) {
      const content = await runtime.getAssetContent(params.assetId)
      if (!content) json(404, { error: '资产不存在。' })
      else json(200, content)
    },
  },
  {
    method: 'GET',
    path: '/api/assets/:assetId/download',
    async handler({ runtime, params, url, res, json }) {
      const download = await runtime.getAssetDownload(params.assetId)
      if (!download) {
        json(404, { error: '资产不存在或不可下载。' })
        return
      }
      res.writeHead(200, {
        'Content-Type': download.asset.mimeType || 'application/octet-stream',
        'Content-Length': download.buffer.length,
        'Content-Disposition': `${url.searchParams.get('inline') === '1' ? 'inline' : 'attachment'}; filename*=UTF-8''${encodeURIComponent(download.asset.name)}`,
        'Cache-Control': 'private, max-age=60',
      })
      res.end(download.buffer)
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
