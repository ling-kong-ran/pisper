import assert from 'node:assert/strict'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'

function request(method, body) {
  return {
    method,
    async *[Symbol.asyncIterator]() {
      if (body !== undefined) yield Buffer.from(JSON.stringify(body))
    },
  }
}

function response() {
  return {
    status: 0,
    body: '',
    destroyed: false,
    writableEnded: false,
    writeHead(status) {
      this.status = status
    },
    flushHeaders() {},
    write(body = '') {
      this.body += body
    },
    end(body = '') {
      this.body += body
      this.writableEnded = true
    },
  }
}

test('app update API distinguishes upstream failures from invalid requests', async () => {
  const updates = {
    async check() {
      throw new Error('GitHub commit 比较失败：HTTP 403')
    },
  }
  const handler = createApiHandler({}, { updates })
  const output = response()

  assert.equal(
    await handler(request('GET'), output, new URL('http://localhost/api/app-update?refresh=1')),
    true,
  )
  assert.equal(output.status, 502)
  assert.deepEqual(JSON.parse(output.body), { error: 'GitHub commit 比较失败：HTTP 403' })
})

test('runtime diagnostics API returns sidecar memory counters', async () => {
  const diagnostics = {
    memory: { rss: 123, heapUsed: 45 },
    sessions: { resident: 1, idle: 1 },
    historyCache: { entries: 0 },
  }
  const handler = createApiHandler({ getRuntimeDiagnostics: () => diagnostics })
  const output = response()

  assert.equal(
    await handler(request('GET'), output, new URL('http://localhost/api/runtime/diagnostics')),
    true,
  )
  assert.equal(output.status, 200)
  assert.deepEqual(JSON.parse(output.body), diagnostics)
})

test('compaction preference APIs expose and update the threshold percentage', async () => {
  const calls = []
  const runtime = {
    getCompactionPreference() {
      calls.push(['get'])
      return { thresholdPercent: 80, minPercent: 50, maxPercent: 95 }
    },
    async updateCompactionPreference(input) {
      calls.push(['update', input.thresholdPercent])
      return { thresholdPercent: input.thresholdPercent, minPercent: 50, maxPercent: 95 }
    },
  }
  const handler = createApiHandler(runtime)

  const getResponse = response()
  assert.equal(
    await handler(request('GET'), getResponse, new URL('http://localhost/api/settings/compaction')),
    true,
  )
  assert.deepEqual(JSON.parse(getResponse.body), {
    thresholdPercent: 80,
    minPercent: 50,
    maxPercent: 95,
  })

  const patchResponse = response()
  assert.equal(
    await handler(
      request('PATCH', { thresholdPercent: 75 }),
      patchResponse,
      new URL('http://localhost/api/settings/compaction'),
    ),
    true,
  )
  assert.deepEqual(JSON.parse(patchResponse.body), {
    thresholdPercent: 75,
    minPercent: 50,
    maxPercent: 95,
  })
  assert.deepEqual(calls, [['get'], ['update', 75]])
})

test('manual session compaction API returns the runtime projection', async () => {
  const calls = []
  const result = {
    compaction: { status: 'completed', reason: 'manual', tokensSaved: 800 },
    contextUsage: { tokens: 200, contextWindow: 10_000, percent: 2 },
  }
  const runtime = {
    async compactSession(sessionId) {
      calls.push(sessionId)
      return result
    },
  }
  const handler = createApiHandler(runtime)
  const output = response()

  assert.equal(
    await handler(
      request('POST', {}),
      output,
      new URL('http://localhost/api/sessions/session-1/compact'),
    ),
    true,
  )
  assert.equal(output.status, 200)
  assert.deepEqual(JSON.parse(output.body), result)
  assert.deepEqual(calls, ['session-1'])
})

test('skills APIs forward the active session scope for global and project discovery', async () => {
  const calls = []
  const runtime = {
    async getSkillsDashboard(sessionId) {
      calls.push(['dashboard', sessionId])
      return { cwd: '/project', skills: [] }
    },
    async installSkill(input, sessionId) {
      calls.push(['install', input.source, sessionId])
      return { skills: [] }
    },
    async reloadSkills(sessionId) {
      calls.push(['reload', sessionId])
      return { skills: [] }
    },
    async updateSkill(id, input, sessionId) {
      calls.push(['update', id, input.enabled, sessionId])
      return { id, enabled: input.enabled }
    },
    async deleteSkill(id, sessionId) {
      calls.push(['delete', id, sessionId])
      return true
    },
  }
  const handler = createApiHandler(runtime)
  const sessionQuery = '?sessionId=session%201'

  const dashboardResponse = response()
  await handler(
    request('GET'),
    dashboardResponse,
    new URL(`http://localhost/api/skills${sessionQuery}`),
  )
  assert.equal(JSON.parse(dashboardResponse.body).cwd, '/project')

  await handler(
    request('POST', { source: './skill' }),
    response(),
    new URL(`http://localhost/api/skills/install${sessionQuery}`),
  )
  await handler(
    request('POST', {}),
    response(),
    new URL(`http://localhost/api/skills/reload${sessionQuery}`),
  )
  await handler(
    request('PATCH', { enabled: false }),
    response(),
    new URL(`http://localhost/api/skills/project-helper${sessionQuery}`),
  )
  await handler(
    request('DELETE'),
    response(),
    new URL(`http://localhost/api/skills/global-helper${sessionQuery}`),
  )

  assert.deepEqual(calls, [
    ['dashboard', 'session 1'],
    ['install', './skill', 'session 1'],
    ['reload', 'session 1'],
    ['update', 'project-helper', false, 'session 1'],
    ['delete', 'global-helper', 'session 1'],
  ])
})

test('chat API forwards explicit Tool requests as structured runtime input', async () => {
  const calls = []
  const runtime = {
    async streamPrompt(input) {
      calls.push(input)
      input.send('done', { text: 'complete' })
    },
  }
  const handler = createApiHandler(runtime)
  const output = response()

  assert.equal(
    await handler(
      request('POST', {
        sessionId: 'session-1',
        message: '/web_search Pisper release',
        requestedToolNames: ['web_search'],
      }),
      output,
      new URL('http://localhost/api/chat'),
    ),
    true,
  )

  assert.equal(output.status, 200)
  assert.deepEqual(calls[0].requestedToolNames, ['web_search'])
  assert.match(output.body, /event: done/)
})

test('session creation forwards the CLI workspace to the shared runtime', async () => {
  const calls = []
  const runtime = {
    async createSession(name, cwd) {
      calls.push([name, cwd])
      return { id: 'session-1', name, cwd }
    },
  }
  const handler = createApiHandler(runtime)
  const output = response()

  assert.equal(
    await handler(
      request('POST', { name: 'CLI chat', cwd: 'E:\\code\\workspace' }),
      output,
      new URL('http://localhost/api/sessions'),
    ),
    true,
  )
  assert.equal(output.status, 201)
  assert.deepEqual(calls, [['CLI chat', 'E:\\code\\workspace']])
})

test('session model and thinking APIs delegate current-session changes to the runtime', async () => {
  const calls = []
  const runtime = {
    async setSessionModel(id, provider, model) {
      calls.push(['model', id, provider, model])
      return { id, model: `${provider}/${model}` }
    },
    async getSessionThinkingState(id) {
      calls.push(['thinking-state', id])
      return {
        id,
        thinkingLevel: 'off',
        availableLevels: ['off', 'xhigh', 'max'],
        status: 'supported',
        message: '',
        model: 'relay/gpt-5.6-sol',
      }
    },
    async setSessionThinkingLevel(id, level) {
      calls.push(['thinking', id, level])
      return {
        id,
        thinkingLevel: level,
        availableLevels: ['off', 'xhigh', 'max'],
        status: 'supported',
        message: '',
        model: 'relay/gpt-5.6-sol',
      }
    },
  }
  const handler = createApiHandler(runtime)

  const modelResponse = response()
  assert.equal(
    await handler(
      request('PUT', { provider: 'openai', model: 'gpt-5.6' }),
      modelResponse,
      new URL('http://localhost/api/sessions/session%201/model'),
    ),
    true,
  )
  assert.deepEqual(JSON.parse(modelResponse.body), { id: 'session 1', model: 'openai/gpt-5.6' })

  const thinkingStateResponse = response()
  assert.equal(
    await handler(
      request('GET'),
      thinkingStateResponse,
      new URL('http://localhost/api/sessions/session%201/thinking-level'),
    ),
    true,
  )
  assert.deepEqual(JSON.parse(thinkingStateResponse.body), {
    id: 'session 1',
    thinkingLevel: 'off',
    availableLevels: ['off', 'xhigh', 'max'],
    status: 'supported',
    message: '',
    model: 'relay/gpt-5.6-sol',
  })

  const thinkingResponse = response()
  assert.equal(
    await handler(
      request('PUT', { level: 'xhigh' }),
      thinkingResponse,
      new URL('http://localhost/api/sessions/session%201/thinking-level'),
    ),
    true,
  )
  assert.deepEqual(JSON.parse(thinkingResponse.body), {
    id: 'session 1',
    thinkingLevel: 'xhigh',
    availableLevels: ['off', 'xhigh', 'max'],
    status: 'supported',
    message: '',
    model: 'relay/gpt-5.6-sol',
  })
  assert.deepEqual(calls, [
    ['model', 'session 1', 'openai', 'gpt-5.6'],
    ['thinking-state', 'session 1'],
    ['thinking', 'session 1', 'xhigh'],
  ])
})

test('session VCS APIs delegate Git/SVN change actions to the runtime', async () => {
  const calls = []
  const runtime = {
    async getSessionVcsChanges(id) {
      calls.push(['changes', id])
      return { id, vcs: 'svn', isRepo: true, files: [], diff: '' }
    },
    async commitSessionVcsChanges(id, message) {
      calls.push(['commit', id, message])
      return { id, vcs: 'svn', isRepo: true, files: [], diff: '' }
    },
    async pushSessionVcsChanges(id) {
      calls.push(['push', id])
      return { id, vcs: 'git', isRepo: true, files: [], diff: '' }
    },
    async revertSessionVcsChanges(id) {
      calls.push(['revert', id])
      return { id, vcs: 'svn', isRepo: true, files: [], diff: '' }
    },
  }
  const handler = createApiHandler(runtime)
  for (const [method, path, body] of [
    ['GET', '/api/sessions/session-vcs/vcs/changes'],
    ['POST', '/api/sessions/session-vcs/vcs/commit', { message: 'SVN changes' }],
    ['POST', '/api/sessions/session-vcs/vcs/push', {}],
    ['POST', '/api/sessions/session-vcs/vcs/revert', {}],
  ]) {
    const output = response()
    assert.equal(
      await handler(request(method, body), output, new URL(`http://localhost${path}`)),
      true,
    )
    assert.equal(output.status, 200)
  }
  assert.deepEqual(calls, [
    ['changes', 'session-vcs'],
    ['commit', 'session-vcs', 'SVN changes'],
    ['push', 'session-vcs'],
    ['revert', 'session-vcs'],
  ])
})

test('session execution mode API delegates to the runtime', async () => {
  const calls = []
  const runtime = {
    async setSessionExecutionMode(id, mode) {
      calls.push(['mode', id, mode])
      return { id, executionMode: mode, permissionMode: mode === 'full-access' ? 'ignore' : 'auto' }
    },
  }
  const handler = createApiHandler(runtime)

  const modeResponse = response()
  assert.equal(
    await handler(
      request('PUT', { mode: 'full-access' }),
      modeResponse,
      new URL('http://localhost/api/sessions/session%201/execution-mode'),
    ),
    true,
  )
  assert.equal(modeResponse.status, 200)
  assert.deepEqual(JSON.parse(modeResponse.body), {
    id: 'session 1',
    executionMode: 'full-access',
    permissionMode: 'ignore',
  })
  assert.deepEqual(calls, [['mode', 'session 1', 'full-access']])
})
