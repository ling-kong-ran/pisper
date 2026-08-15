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

test('memory preference APIs expose and update the auto-approve confidence', async () => {
  const calls = []
  const runtime = {
    getMemoryPreference() {
      calls.push(['get'])
      return { autoApproveConfidence: 60, minConfidence: 0, maxConfidence: 100 }
    },
    async updateMemoryPreference(input) {
      calls.push(['update', input.autoApproveConfidence])
      return {
        autoApproveConfidence: input.autoApproveConfidence,
        minConfidence: 0,
        maxConfidence: 100,
      }
    },
  }
  const handler = createApiHandler(runtime)

  const getResponse = response()
  assert.equal(
    await handler(request('GET'), getResponse, new URL('http://localhost/api/settings/memory')),
    true,
  )
  assert.deepEqual(JSON.parse(getResponse.body), {
    autoApproveConfidence: 60,
    minConfidence: 0,
    maxConfidence: 100,
  })

  const patchResponse = response()
  assert.equal(
    await handler(
      request('PATCH', { autoApproveConfidence: 75 }),
      patchResponse,
      new URL('http://localhost/api/settings/memory'),
    ),
    true,
  )
  assert.deepEqual(JSON.parse(patchResponse.body), {
    autoApproveConfidence: 75,
    minConfidence: 0,
    maxConfidence: 100,
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

test('session derivation API forwards the persisted turn boundary and display name', async () => {
  const calls = []
  const result = { id: 'derived-1', name: 'Source · Derived' }
  const runtime = {
    async deriveSession(sessionId, boundaryEntryId, name) {
      calls.push({ sessionId, boundaryEntryId, name })
      return result
    },
  }
  const handler = createApiHandler(runtime)
  const output = response()

  assert.equal(
    await handler(
      request('POST', { boundaryEntryId: 'entry-1', name: 'Source · Derived' }),
      output,
      new URL('http://localhost/api/sessions/source-1/derive'),
    ),
    true,
  )
  assert.equal(output.status, 201)
  assert.deepEqual(JSON.parse(output.body), result)
  assert.deepEqual(calls, [
    { sessionId: 'source-1', boundaryEntryId: 'entry-1', name: 'Source · Derived' },
  ])
})

test('session label search API returns the runtime-owned cross-session index', async () => {
  const calls = []
  const runtime = {
    async searchSessionTreeLabels(query, options) {
      calls.push([query, options])
      return [{ sessionId: 'session-1', entryId: 'entry-1', label: 'Checkpoint' }]
    },
  }
  const handler = createApiHandler(runtime)
  const output = response()

  assert.equal(
    await handler(
      request('GET'),
      output,
      new URL('http://localhost/api/session-labels?query=check&limit=12'),
    ),
    true,
  )
  assert.equal(output.status, 200)
  assert.deepEqual(JSON.parse(output.body), {
    labels: [{ sessionId: 'session-1', entryId: 'entry-1', label: 'Checkpoint' }],
  })
  assert.deepEqual(calls, [['check', { limit: '12' }]])
})

test('session tree APIs read, navigate, and label Pi tree entries', async () => {
  const calls = []
  const tree = { sessionId: 'session 1', leafId: 'leaf-1', nodes: [] }
  const runtime = {
    async getSessionTree(sessionId) {
      calls.push(['get', sessionId])
      return tree
    },
    async navigateSessionTree(sessionId, entryId, options) {
      calls.push(['navigate', sessionId, entryId, options])
      const navigation = { editorText: 'Edit me', cancelled: false }
      return options.includeTree === false ? navigation : { ...tree, ...navigation }
    },
    async setSessionTreeLabel(sessionId, entryId, label) {
      calls.push(['label', sessionId, entryId, label])
      return tree
    },
  }
  const handler = createApiHandler(runtime)
  const url = (suffix) => new URL(`http://localhost/api/sessions/session%201/tree${suffix}`)

  const getResponse = response()
  assert.equal(await handler(request('GET'), getResponse, url('')), true)
  assert.deepEqual(JSON.parse(getResponse.body), tree)

  const navigateResponse = response()
  assert.equal(
    await handler(
      request('POST', { targetEntryId: 'entry-1', summarize: true }),
      navigateResponse,
      url('/navigate'),
    ),
    true,
  )
  assert.deepEqual(JSON.parse(navigateResponse.body), {
    ...tree,
    editorText: 'Edit me',
    cancelled: false,
  })

  const compactNavigateResponse = response()
  assert.equal(
    await handler(
      request('POST', { targetEntryId: 'entry-2', includeTree: false }),
      compactNavigateResponse,
      url('/navigate'),
    ),
    true,
  )
  assert.deepEqual(JSON.parse(compactNavigateResponse.body), {
    editorText: 'Edit me',
    cancelled: false,
  })

  const labelResponse = response()
  assert.equal(
    await handler(request('PUT', { label: 'Checkpoint' }), labelResponse, url('/labels/entry%202')),
    true,
  )
  assert.deepEqual(JSON.parse(labelResponse.body), tree)
  assert.deepEqual(calls, [
    ['get', 'session 1'],
    ['navigate', 'session 1', 'entry-1', { summarize: true }],
    ['navigate', 'session 1', 'entry-2', { summarize: false, includeTree: false }],
    ['label', 'session 1', 'entry 2', 'Checkpoint'],
  ])
})

test('workspace trust APIs resolve and persist a session-scoped decision', async () => {
  const calls = []
  const runtime = {
    async getWorkspaceTrust(sessionId) {
      calls.push(['get', sessionId])
      return { cwd: '/project', decision: null, requiresDecision: true }
    },
    async setWorkspaceTrust(sessionId, trusted) {
      calls.push(['set', sessionId, trusted])
      return { cwd: '/project', decision: trusted, trusted }
    },
  }
  const handler = createApiHandler(runtime)

  const getResponse = response()
  assert.equal(
    await handler(
      request('GET'),
      getResponse,
      new URL('http://localhost/api/sessions/session%201/workspace-trust'),
    ),
    true,
  )
  assert.deepEqual(JSON.parse(getResponse.body), {
    cwd: '/project',
    decision: null,
    requiresDecision: true,
  })

  const putResponse = response()
  assert.equal(
    await handler(
      request('PUT', { trusted: true }),
      putResponse,
      new URL('http://localhost/api/sessions/session%201/workspace-trust'),
    ),
    true,
  )
  assert.deepEqual(JSON.parse(putResponse.body), {
    cwd: '/project',
    decision: true,
    trusted: true,
  })
  assert.deepEqual(calls, [
    ['get', 'session 1'],
    ['set', 'session 1', true],
  ])
})

test('session commands API returns the Runtime-authoritative Slash command catalog', async () => {
  const calls = []
  const catalog = {
    sessionId: 'session 1',
    commands: [{ name: 'review', invocation: '/review', source: 'prompt' }],
    counts: { total: 1, prompts: 1, skills: 0, diagnostics: 0 },
  }
  const runtime = {
    async getSessionCommands(sessionId) {
      calls.push(sessionId)
      return catalog
    },
  }
  const handler = createApiHandler(runtime)
  const output = response()

  assert.equal(
    await handler(
      request('GET'),
      output,
      new URL('http://localhost/api/sessions/session%201/commands'),
    ),
    true,
  )
  assert.deepEqual(JSON.parse(output.body), catalog)
  assert.deepEqual(calls, ['session 1'])
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

test('plugins API forwards the active session scope for callable Tool filtering', async () => {
  const calls = []
  const runtime = {
    async getPlugins(sessionId) {
      calls.push(sessionId)
      return { plugins: [], callableToolNames: ['read'] }
    },
  }
  const handler = createApiHandler(runtime)
  const output = response()

  await handler(
    request('GET'),
    output,
    new URL('http://localhost/api/plugins?sessionId=session%201'),
  )

  assert.deepEqual(calls, ['session 1'])
  assert.deepEqual(JSON.parse(output.body).callableToolNames, ['read'])
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

test('chat API dispatches structured Skill, Tool, and workflow invocations', async () => {
  const prompts = []
  const workflows = []
  const runtime = {
    async streamPrompt(input) {
      prompts.push(input)
      input.send('done', { text: 'complete' })
    },
    async runWorkflow(id, options) {
      workflows.push([id, options])
      return { run: { id: 'run-1', status: 'running' } }
    },
  }
  const handler = createApiHandler(runtime)
  const skillOutput = response()
  await handler(
    request('POST', {
      sessionId: 'session-1',
      message: '检查这个改动',
      invocation: { kind: 'skill', resourceId: 'review', resourceName: 'code-review' },
    }),
    skillOutput,
    new URL('http://localhost/api/chat'),
  )
  assert.equal(prompts.length, 1)
  assert.equal(prompts[0].message, '/skill:code-review\n检查这个改动')

  const toolOutput = response()
  await handler(
    request('POST', {
      sessionId: 'session-1',
      message: '查看当前项目信息',
      invocation: {
        kind: 'tool',
        resourceId: 'project_package_info',
        resourceName: 'Project Package Info',
      },
    }),
    toolOutput,
    new URL('http://localhost/api/chat'),
  )
  assert.equal(prompts.length, 2)
  assert.equal(prompts[1].message, '查看当前项目信息')
  assert.deepEqual(prompts[1].requestedToolNames, ['project_package_info'])

  const workflowOutput = response()
  await handler(
    request('POST', {
      sessionId: 'session-1',
      message: '准备发布',
      invocation: {
        kind: 'workflow',
        resourceId: 'release',
        resourceName: '发布准备',
        arguments: { channel: 'beta' },
      },
    }),
    workflowOutput,
    new URL('http://localhost/api/chat'),
  )
  assert.deepEqual(workflows, [
    [
      'release',
      {
        trigger: 'chat',
        inputs: { channel: 'beta' },
        sourceSessionId: 'session-1',
        sourceMessage: '准备发布',
      },
    ],
  ])
  assert.match(workflowOutput.body, /event: invocation_started/)
  assert.match(workflowOutput.body, /"runId":"run-1"/)
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
