import assert from 'node:assert/strict'

// 调用方必须使用隔离 agent/workspace 与本地模拟模型；成功搜索仍发送到真实固定 Bing 端点。
export async function checkWebSearchParity({
  check,
  json,
  request,
  chat,
  workspace,
  providerId,
  modelId,
  liveBing = false,
}) {
  const before = await json('/api/plugins')
  let owned
  try {
    await check('Web search catalog and config follow the release Bing contract', async () => {
      assert.ok(before.tools.some((tool) => tool.id === 'web_search'))
      const descriptor = before.tools.find((tool) => tool.id === 'web_search')
      assert.equal(descriptor.source, 'app')
      assert.equal(descriptor.risk, 'medium')
      assert.equal(descriptor.scope, 'Bing public web search')
      const result = await json('/api/plugins', 'PUT', {
        enabledTools: [...new Set([...before.enabledTools, 'web_search'])],
        webSearch: {
          provider: 'ignored',
          language: ' \uFEFFko-KR\u00A0 ',
          safeSearch: '1.5',
          maxResults: '0x6',
        },
      })
      assert.deepEqual(result.webSearch, {
        provider: 'bing',
        language: 'ko-KR',
        safeSearch: 2,
        maxResults: 6,
      })
      assert.ok(result.enabledTools.includes('web_search'))
      const reloaded = await json('/api/plugins')
      assert.deepEqual(reloaded.webSearch, result.webSearch)
      return { provider: 'bing', savedLanguage: 'ko-KR', savedMaxResults: 6 }
    })

    owned = await json('/api/sessions', 'POST', {
      name: 'web-search-native-fixture',
      cwd: workspace,
    })
    assert.ok(owned.id)
    await json(`/api/sessions/${owned.id}/model`, 'PUT', { provider: providerId, model: modelId })
    await json(`/api/sessions/${owned.id}/execution-mode`, 'PUT', { mode: 'full-access' })

    await check(
      'Actual Pi web_search emits a progress update and a native validation error',
      async () => {
        const payload = Buffer.from(
          JSON.stringify({
            name: 'call_tool',
            args: { name: 'web_search', arguments: { query: ' ' } },
          }),
        ).toString('base64')
        const events = await chat(`rust-snapshot-tool:${payload}`, owned.id)
        const started = events.find(
          (event) => event.event === 'tool_start' && event.data.name === 'call_tool',
        )
        assert.ok(started)
        assert.equal(started.data.args.name, 'web_search')
        const updates = events.filter(
          (event) => event.event === 'tool_update' && event.data.id === started.data.id,
        )
        assert.ok(
          updates.some((event) => JSON.stringify(event.data).includes('Searching Bing for: ')),
        )
        const completed = events.find(
          (event) => event.event === 'tool_end' && event.data.id === started.data.id,
        )
        assert.ok(completed, JSON.stringify(events))
        assert.equal(completed.data.error, true)
        assert.ok(JSON.stringify(completed.data).includes('搜索关键词不能为空。'))
        return {
          tool: 'web_search',
          gateway: 'call_tool',
          streamedUpdate: true,
          nativeValidation: 'empty-query',
          sentToBing: false,
        }
      },
    )

    if (liveBing) {
      await check(
        'Live Bing connectivity test returns the release count/provider response',
        async () => {
          const response = await request('/api/plugins/web-search/test', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ language: 'en-US', safeSearch: 1, maxResults: 8 }),
            signal: AbortSignal.timeout(20000),
          })
          const value = await response.json()
          assert.equal(response.status, 200, JSON.stringify(value))
          assert.deepEqual(Object.keys(value).sort(), ['count', 'provider'])
          assert.equal(value.provider, 'bing')
          assert.ok(Number.isInteger(value.count) && value.count >= 0 && value.count <= 3)
          return value
        },
      )
      await check('Actual Pi web_search returns live Bing result details', async () => {
        const payload = Buffer.from(
          JSON.stringify({
            name: 'call_tool',
            args: {
              name: 'web_search',
              arguments: { query: 'Pisper AI agent', language: 'en-US', limit: 3 },
            },
          }),
        ).toString('base64')
        const events = await chat(`rust-snapshot-tool:${payload}`, owned.id)
        const started = events.find(
          (event) => event.event === 'tool_start' && event.data.name === 'call_tool',
        )
        assert.ok(started)
        const completed = events.find(
          (event) => event.event === 'tool_end' && event.data.id === started.data.id,
        )
        assert.ok(completed, JSON.stringify(events))
        assert.equal(completed.data.error, false)
        const details = completed.data.result?.details
        assert.equal(details?.gatewayToolName, 'web_search')
        assert.equal(details?.provider, 'bing')
        assert.equal(details.query, 'Pisper AI agent')
        assert.ok(Array.isArray(details.results) && details.results.length <= 3)
        for (const result of details.results) {
          assert.ok(['http:', 'https:'].includes(new URL(result.url).protocol))
          for (const field of ['title', 'snippet', 'publishedAt'])
            assert.equal(typeof result[field], 'string')
        }
        assert.equal(typeof details.text, 'string')
        return { provider: details.provider, count: details.results.length }
      })
    }
  } finally {
    if (owned) {
      await json(`/api/sessions/${owned.id}/abort`, 'POST', {})
      const response = await request(`/api/sessions/${owned.id}`, { method: 'DELETE' })
      assert.ok(
        [200, 404].includes(response.status),
        `Owned web-search session cleanup: ${response.status}`,
      )
    }
    await json('/api/plugins', 'PUT', {
      enabledTools: before.enabledTools,
      webSearch: before.webSearch,
    })
  }
}
