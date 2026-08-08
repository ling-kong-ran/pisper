import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { defineTool, SessionManager } from '@earendil-works/pi-coding-agent'
import { Type } from 'typebox'
import {
  AgentRuntimeService,
  storedSessionModel,
  storedSessionModelId,
} from '../runtime/agent-runtime.mjs'

test('blank chat sessions stay lightweight until an Agent is first required', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-pending-session-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = {
    getGlobalSettings: () => ({
      defaultProvider: 'openai',
      defaultModel: 'gpt-test',
      defaultThinkingLevel: 'high',
    }),
  }
  runtime.saveSessionMeta = async () => {}
  runtime.listStoredSessions = async () => []
  let runtimeCreations = 0
  runtime.createSessionRuntime = async (manager, name) => {
    runtimeCreations += 1
    return { manager, name, created: 'runtime-created', modified: 'runtime-modified' }
  }
  const workspace = join(directory, 'cli-workspace')
  await mkdir(workspace)

  const created = await runtime.createSession('Pending chat', workspace)
  assert.equal(runtimeCreations, 0)
  assert.equal(created.cwd, workspace)
  assert.equal(runtime.pendingSessions.has(created.id), true)
  assert.equal(created.model, 'openai/gpt-test')
  assert.equal(created.thinkingLevel, 'high')
  const [pending] = await runtime.listSessions()
  assert.equal(pending.name, 'Pending chat')
  assert.equal(pending.thinkingLevel, 'high')

  await runtime.renameSession(created.id, 'Renamed pending chat')
  await runtime.setSessionCwd(created.id, directory)
  const activated = await runtime.getOrCreateSession(created.id)

  assert.equal(runtimeCreations, 1)
  assert.equal(runtime.pendingSessions.has(created.id), false)
  assert.equal(activated.manager.getSessionId(), created.id)
  assert.equal(activated.name, 'Renamed pending chat')
  assert.equal(activated.created, created.created)
})

test('main runtime keeps discovered cold MCP tools for the rest of the session while child resources remain available', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-runtime-resources-'))
  let runtime
  t.after(async () => {
    await runtime?.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await mkdir(join(directory, 'skills', 'runtime-skill'), { recursive: true })
  await writeFile(
    join(directory, 'skills', 'runtime-skill', 'SKILL.md'),
    `---\nname: runtime-skill\ndescription: Verify runtime skill loading.\n---\n\nUse this runtime skill.\n`,
    'utf8',
  )

  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  assert.equal(runtime.plans.path, join(directory, 'pisper-plans.json'))
  assert.equal(runtime.plans.legacyPath, join(directory, 'pisper-task-lists.json'))
  await runtime.init()
  runtime.mcp.createToolDefinitions = async () => [
    defineTool({
      name: 'mcp_fixture_echo_12345678',
      label: 'MCP fixture echo',
      description: 'Fixture MCP tool',
      parameters: Type.Object({ text: Type.String() }),
      async execute(_id, params) {
        return { content: [{ type: 'text', text: params.text }], details: {} }
      },
    }),
  ]

  const value = await runtime.createSessionRuntime(SessionManager.inMemory(directory))
  assert.ok(
    value.session.resourceLoader.getSkills().skills.some((skill) => skill.name === 'runtime-skill'),
  )
  assert.ok(value.session.agent.state.systemPrompt.includes('runtime-skill'))
  assert.match(value.session.agent.state.systemPrompt, /Application: Pisper/)
  assert.match(value.session.agent.state.systemPrompt, /Active model:/)
  assert.doesNotMatch(value.session.agent.state.systemPrompt, /You are Pisper/i)
  assert.equal(value.session.getActiveToolNames().includes('mcp_fixture_echo_12345678'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_list'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_manage'), false)
  assert.ok(value.session.getActiveToolNames().includes('read'))
  assert.ok(value.session.getActiveToolNames().includes('edit'))
  assert.ok(value.session.getActiveToolNames().includes('write'))
  assert.ok(value.session.getActiveToolNames().includes('bash'))
  assert.ok(value.session.getActiveToolNames().includes('get_plan'))
  assert.ok(value.session.getActiveToolNames().includes('update_plan'))
  assert.equal(value.session.getActiveToolNames().includes('get_task_list'), false)
  assert.equal(value.session.getActiveToolNames().includes('update_task_list'), false)
  assert.ok(value.session.getToolDefinition('get_task_list'))
  assert.ok(value.session.getToolDefinition('update_task_list'))
  const compatibilityRead = await value.session
    .getToolDefinition('get_task_list')
    .execute('legacy-plan-read', {}, new AbortController().signal)
  assert.equal(compatibilityRead.details.plan.sessionId, value.session.sessionId)
  assert.equal(Object.hasOwn(compatibilityRead.details, 'taskList'), false)
  assert.doesNotMatch(value.session.agent.state.systemPrompt, /get_task_list|update_task_list/)
  assert.ok(value.session.getActiveToolNames().includes('discover_tools'))
  assert.equal(value.session.getActiveToolNames().includes('generate_visual'), false)
  assert.ok(value.session.getActiveToolNames().includes('call_tool'))
  assert.match(value.session.agent.state.systemPrompt, /discover_tools/)
  assert.match(value.session.getToolDefinition('generate_visual').description, /mockup/)
  assert.deepEqual(
    value.session.getToolDefinition('generate_visual').parameters.properties.aspectRatio.enum,
    ['1:1', '16:9', '9:16', '4:3', '3:4'],
  )
  assert.equal(
    value.session.getToolDefinition('generate_visual').parameters.properties.aspectRatio.anyOf,
    undefined,
  )
  const hotToolNames = value.session.getActiveToolNames()
  const hotSystemPrompt = value.session.agent.state.systemPrompt

  await runtime.selectToolsForMessage(value, '先给我来个设计图，我看看样式是什么样的。')
  assert.deepEqual(value.requestedToolNames, [])
  assert.equal(value.session.getActiveToolNames().includes('generate_visual'), false)
  assert.deepEqual(value.promotedToolNames, [])
  assert.equal(value.session.agent.state.systemPrompt, hotSystemPrompt)

  const discovery = await value.session
    .getToolDefinition('discover_tools')
    .execute(
      'discover-fixture',
      { query: 'MCP fixture echo', limit: 1 },
      new AbortController().signal,
    )
  assert.equal(discovery.details.activated, undefined)
  assert.equal(value.session.getActiveToolNames().includes('mcp_fixture_echo_12345678'), false)
  assert.match(discovery.content[0].text, /call_tool/)

  await runtime.selectToolsForMessage(value, 'Use the MCP fixture echo tool for this task.')
  assert.equal(value.session.getActiveToolNames().includes('mcp_fixture_echo_12345678'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_list'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_manage'), false)
  assert.deepEqual(value.session.getActiveToolNames().slice(0, hotToolNames.length), hotToolNames)
  assert.equal(value.session.agent.state.systemPrompt, hotSystemPrompt)
  assert.match(
    value.session.getToolDefinition('mcp_manage').description,
    /Always use mcp_manage for MCP configuration/,
  )
  assert.deepEqual(value.session.getToolDefinition('mcp_manage').promptGuidelines, [])

  await runtime.selectToolsForMessage(value, 'Now update the local source file.')
  assert.equal(value.session.getActiveToolNames().includes('mcp_fixture_echo_12345678'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_list'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_manage'), false)
  assert.deepEqual(value.promotedToolNames, [])
  assert.equal(runtime.sessionMeta[value.session.sessionId]?.promotedToolNames, undefined)
  assert.equal(value.session.hasExtensionHandlers('tool_result'), false)
  assert.equal(value.session.hasExtensionHandlers('message_end'), false)

  value.isolatedContext = true
  value.blockedToolNames = ['memory_search', 'memory_remember']
  await runtime.selectToolsForMessage(value, '搜索星忆并记住这项信息。')
  assert.equal(value.session.getActiveToolNames().includes('memory_search'), false)
  assert.equal(value.session.getActiveToolNames().includes('memory_remember'), false)

  const compatibilityDiscovery = await value.session
    .getToolDefinition('discover_tools')
    .execute(
      'discover-legacy-plan-alias',
      { query: 'get_task_list update_task_list', limit: 5, activate: false },
      new AbortController().signal,
    )
  assert.equal(
    compatibilityDiscovery.details.matches.some((tool) =>
      ['get_task_list', 'update_task_list'].includes(tool.name),
    ),
    false,
  )
  await runtime.selectToolsForMessage(value, 'Legacy client plan request.', {
    requestedToolNames: ['get_task_list', 'update_task_list'],
  })
  assert.equal(value.session.getActiveToolNames().includes('get_task_list'), false)
  assert.equal(value.session.getActiveToolNames().includes('update_task_list'), false)
  assert.deepEqual(value.requestedToolNames, ['get_task_list'])
  assert.doesNotMatch(value.session.agent.state.systemPrompt, /get_task_list|update_task_list/)
  await runtime.selectToolsForMessage(value, 'Return to canonical tools.')
  assert.deepEqual(value.requestedToolNames, [])

  const childLoader = await runtime.multiAgents.createResourceLoader({
    cwd: directory,
    appendSystemPrompt: 'CHILD AGENT PROMPT',
  })
  assert.ok(childLoader.getSkills().skills.some((skill) => skill.name === 'runtime-skill'))
  assert.ok(childLoader.getAppendSystemPrompt().includes('CHILD AGENT PROMPT'))
})

test('explicit client tool requests activate only the structured tool names', async () => {
  const promoted = []
  const runtime = Object.create(AgentRuntimeService.prototype)
  runtime.promoteSessionTools = async (_value, names) => {
    promoted.push(...names)
    return { routedToolNames: names }
  }
  const value = {
    requestedToolNames: [],
    session: { getActiveToolNames: () => ['read', 'web_search'] },
  }

  const active = await runtime.selectToolsForMessage(value, '/web_search Pisper releases', {
    requestedToolNames: ['web_search', '', 'web_search'],
  })

  assert.deepEqual(value.requestedToolNames, ['web_search'])
  assert.deepEqual(promoted, ['web_search'])
  assert.deepEqual(active, ['read', 'web_search'])
})

test('background prompts apply their explicit execution mode before the Agent starts', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-background-prompt-'))
  let runtime
  t.after(async () => {
    await runtime?.dispose?.().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  const calls = []
  runtime.createSession = async () => {
    runtime.sessions.set('scheduled-session', { cwd: directory, session: { model: null } })
    return { id: 'scheduled-session' }
  }
  runtime.setSessionCwd = async (_id, cwd) => {
    calls.push(['cwd', cwd])
  }
  runtime.setSessionExecutionMode = async (_id, mode) => {
    calls.push(['executionMode', mode])
  }
  runtime.streamPrompt = async ({ sessionId, isolatedContext, send }) => {
    calls.push(['stream', sessionId, isolatedContext])
    send('meta', { sessionId })
    send('text_delta', { delta: 'done' })
  }

  const result = await runtime.promptFromChannel({
    sessionId: '',
    message: 'run',
    cwd: directory,
    title: 'Scheduled task',
    executionMode: 'full-access',
    isolatedContext: true,
  })

  assert.deepEqual(
    calls.map(([type]) => type),
    ['cwd', 'executionMode', 'stream'],
  )
  assert.equal(calls.at(-1)[2], true)
  assert.equal(runtime.sessions.get('scheduled-session').isolatedContext, true)
  assert.deepEqual(runtime.sessions.get('scheduled-session').blockedToolNames, [
    'memory_search',
    'memory_remember',
  ])
  assert.equal(result.sessionId, 'scheduled-session')
  assert.equal(result.text, 'done')
})

test('saving plugin tools keeps the current streaming session alive and invalidates idle runtimes', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-plugin-save-'))
  let runtime
  t.after(async () => {
    await runtime?.dispose?.().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  let streamingDisposed = 0
  let idleDisposed = 0
  runtime.sessions.set('streaming', {
    runtimeVersion: 0,
    session: {
      isStreaming: true,
      dispose: () => {
        streamingDisposed += 1
      },
    },
  })
  runtime.sessions.set('idle', {
    runtimeVersion: 0,
    session: {
      isStreaming: false,
      dispose: () => {
        idleDisposed += 1
      },
    },
  })
  runtime.toolPlugins.saveState = async () => ({ enabledTools: ['read'] })
  runtime.pauseSessionGoal = async () => {}
  runtime.multiAgents.abortParent = () => {}
  runtime.permissions.resolveSession = () => {}

  const result = await runtime.savePlugins({ enabledTools: ['read'] })

  assert.deepEqual(result.enabledTools, ['read'])
  assert.equal(runtime.sessionRuntimeVersion, 1)
  assert.equal(streamingDisposed, 0)
  assert.equal(idleDisposed, 1)
  assert.equal(runtime.sessions.has('streaming'), true)
  assert.equal(runtime.sessions.has('idle'), false)
})

test('saving Provider settings keeps the currently streaming session alive', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-save-'))
  let runtime
  t.after(async () => {
    await runtime?.dispose?.().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.init()
  let streamingDisposed = 0
  let idleDisposed = 0
  const runtimeVersion = runtime.sessionRuntimeVersion
  runtime.sessions.set('streaming', {
    runtimeVersion,
    session: {
      isStreaming: true,
      dispose: () => {
        streamingDisposed += 1
      },
    },
  })
  runtime.sessions.set('idle', {
    runtimeVersion,
    session: {
      isStreaming: false,
      dispose: () => {
        idleDisposed += 1
      },
    },
  })
  runtime.pauseSessionGoal = async () => {}
  runtime.multiAgents.abortParent = () => {}
  runtime.permissions.resolveSession = () => {}

  await runtime.saveConfig({
    provider: 'kimi-coding',
    providerType: 'chat',
    model: 'k3',
    apiKey: 'provider-save-key',
    baseUrl: 'https://api.kimi.com/coding/',
    thinkingLevel: 'medium',
    toolMode: 'read-only',
    setAsDefault: true,
  })

  assert.equal(runtime.sessionRuntimeVersion, runtimeVersion + 1)
  assert.equal(streamingDisposed, 0)
  assert.equal(idleDisposed, 1)
  assert.equal(runtime.sessions.has('streaming'), true)
  assert.equal(runtime.sessions.has('idle'), false)
})

test('resource changes keep the currently streaming session alive', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-resource-change-'))
  let runtime
  t.after(async () => {
    await runtime?.dispose?.().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  let streamingDisposed = 0
  let idleDisposed = 0
  runtime.sessions.set('streaming', {
    runtimeVersion: 0,
    session: {
      isStreaming: true,
      dispose: () => {
        streamingDisposed += 1
      },
    },
  })
  runtime.sessions.set('idle', {
    runtimeVersion: 0,
    session: {
      isStreaming: false,
      dispose: () => {
        idleDisposed += 1
      },
    },
  })
  runtime.mcp.add = async () => ({ services: [] })

  await runtime.createMcpServer({ name: 'fixture' })

  assert.equal(runtime.sessionRuntimeVersion, 1)
  assert.equal(streamingDisposed, 0)
  assert.equal(idleDisposed, 1)
  assert.equal(runtime.sessions.has('streaming'), true)
  assert.equal(runtime.sessions.has('idle'), false)
})

test('resident session runtime limit evicts the least-recent idle session but preserves streaming work', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-runtime-lru-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    runtime.sessions.clear()
    await runtime.dispose().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime.maxResidentSessionRuntimes = 2
  runtime.sessionRuntimeIdleTtlMs = Number.POSITIVE_INFINITY
  const disposed = []
  runtime.sessions.set('old-idle', {
    lastAccessedAt: 1,
    session: { isStreaming: false, dispose: () => disposed.push('old-idle') },
  })
  runtime.sessions.set('streaming', {
    lastAccessedAt: 2,
    session: { isStreaming: true, dispose: () => disposed.push('streaming') },
  })
  runtime.sessions.set('new-idle', {
    lastAccessedAt: 3,
    session: { isStreaming: false, dispose: () => disposed.push('new-idle') },
  })

  assert.equal(runtime.evictIdleSessionRuntimes(), 1)
  assert.deepEqual(disposed, ['old-idle'])
  assert.deepEqual([...runtime.sessions.keys()], ['streaming', 'new-idle'])
})

test('idle session runtime TTL releases inactive contexts and their history cache', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-runtime-ttl-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    runtime.sessions.clear()
    await runtime.dispose().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime.sessionRuntimeIdleTtlMs = 100
  const sessionFile = join(directory, 'idle.jsonl')
  const disposed = []
  runtime.sessions.set('expired', {
    lastAccessedAt: 800,
    session: { isStreaming: false, sessionFile, dispose: () => disposed.push('expired') },
  })
  runtime.sessions.set('recent', {
    lastAccessedAt: 950,
    session: { isStreaming: false, dispose: () => disposed.push('recent') },
  })
  runtime.sessions.set('streaming', {
    lastAccessedAt: 1,
    session: { isStreaming: true, dispose: () => disposed.push('streaming') },
  })
  runtime.sessionHistoryCache.set(sessionFile, { size: 1024 })
  runtime.sessionContextUsageCache.set('expired', { value: {} })

  assert.equal(runtime.evictIdleSessionRuntimes('', 1_000), 1)
  assert.deepEqual(disposed, ['expired'])
  assert.equal(runtime.sessions.has('expired'), false)
  assert.equal(runtime.sessions.has('recent'), true)
  assert.equal(runtime.sessions.has('streaming'), true)
  assert.equal(runtime.sessionHistoryCache.has(sessionFile), false)
  assert.equal(runtime.sessionContextUsageCache.has('expired'), false)
})

test('stored session model lookup does not build the full message context', () => {
  const sessionManager = {
    buildSessionContext() {
      throw new Error('full context should not be built')
    },
    getBranch() {
      return [
        { type: 'model_change', provider: 'openai', modelId: 'gpt-old' },
        {
          type: 'message',
          message: { role: 'assistant', provider: 'openai', model: 'gpt-response' },
        },
        { type: 'model_change', provider: 'openai', modelId: 'gpt-current' },
      ]
    },
  }

  assert.deepEqual(storedSessionModel(sessionManager), {
    provider: 'openai',
    modelId: 'gpt-current',
  })
  assert.equal(storedSessionModelId(sessionManager), 'gpt-current')
})

test('empty sessions still restore the last model change after runtime recreation', () => {
  const sessionManager = {
    buildSessionContext() {
      return {
        messages: [],
        model: { provider: 'xai', modelId: 'grok-4.5' },
        thinkingLevel: 'medium',
      }
    },
    getBranch() {
      return [
        { type: 'model_change', provider: 'openai', modelId: 'gpt-out-of-quota' },
        { type: 'thinking_level_change', thinkingLevel: 'medium' },
        { type: 'model_change', provider: 'xai', modelId: 'grok-4.5' },
      ]
    },
  }

  assert.deepEqual(storedSessionModel(sessionManager), {
    provider: 'xai',
    modelId: 'grok-4.5',
  })
})

test('setSessionModel persists the active model into session metadata', async () => {
  const runtime = {
    sessionMeta: {
      'session-1': { model: 'openai/gpt-out-of-quota' },
    },
    saved: 0,
    async saveSessionMeta() {
      this.saved += 1
    },
    providerPreferences: {
      async setSessionModel(id, provider, modelId) {
        return {
          id,
          model: `${provider}/${modelId}`,
          provider,
          modelId,
        }
      },
    },
  }

  const result = await AgentRuntimeService.prototype.setSessionModel.call(
    runtime,
    'session-1',
    'xai',
    'grok-4.5',
  )

  assert.equal(result.model, 'xai/grok-4.5')
  assert.equal(runtime.sessionMeta['session-1'].model, 'xai/grok-4.5')
  assert.equal(runtime.saved, 1)
})

test('listSessions prefers persisted session metadata over the global default model', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-model-meta-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  try {
    runtime.listStoredSessions = async () => [
      {
        id: 'session-switched',
        path: join(directory, 'session.jsonl'),
        name: 'Switched session',
        firstMessage: '',
        messageCount: 0,
        cwd: directory,
        created: new Date('2026-01-01T00:00:00.000Z'),
        modified: new Date('2026-01-01T00:01:00.000Z'),
      },
    ]
    runtime.settingsManager = {
      getGlobalSettings: () => ({
        defaultProvider: 'openai',
        defaultModel: 'gpt-out-of-quota',
      }),
    }
    runtime.goals = { get: () => null }
    runtime.plans = { get: () => null }
    runtime.multiAgents = { summaries: () => [] }
    runtime.sessionMeta = {
      'session-switched': { model: 'xai/grok-4.5' },
    }
    runtime.openStoredSession = () => {
      throw new Error('inactive history should not be reparsed')
    }

    const sessions = await runtime.listSessions()
    assert.equal(sessions[0].model, 'xai/grok-4.5')
  } finally {
    runtime.sessions.clear()
    await runtime.dispose().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  }
})

test('historical session message pages resolve model from the session file and backfill metadata', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-history-model-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  try {
    const sessionPath = join(directory, 'history.jsonl')
    runtime.findSessionInfo = async (id) =>
      id === 'session-history'
        ? {
            id: 'session-history',
            path: sessionPath,
            name: 'History',
            cwd: directory,
          }
        : null
    runtime.openStoredSession = () => ({
      buildSessionContext() {
        return {
          messages: [
            { role: 'user', content: [{ type: 'text', text: 'hello' }] },
            {
              role: 'assistant',
              content: [{ type: 'text', text: 'world' }],
              provider: 'openai',
              model: 'gpt-out-of-quota',
            },
          ],
          model: { provider: 'xai', modelId: 'grok-4.5' },
          thinkingLevel: 'medium',
        }
      },
    })
    runtime.streamProjection.getSessionHistoryMessages = async () => [
      { id: 'u1', role: 'user', text: 'hello' },
      { id: 'a1', role: 'agent', text: 'world' },
    ]
    runtime.streamProjection.getSessionContextUsage = async () => null
    runtime.sessionMeta = {}
    let saved = 0
    runtime.saveSessionMeta = async () => {
      saved += 1
    }
    runtime.streamProjection.saveSessionMeta = () => runtime.saveSessionMeta()

    const page = await runtime.getSessionMessagePage('session-history', { limit: 20 })
    assert.equal(page.model, 'xai/grok-4.5')
    assert.equal(runtime.sessionMeta['session-history'].model, 'xai/grok-4.5')
    assert.equal(saved, 1)
  } finally {
    runtime.sessions.clear()
    await runtime.dispose().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  }
})

test('runtime diagnostics expose bounded memory and cache counters without session content', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-runtime-diagnostics-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    runtime.sessions.clear()
    await runtime.dispose().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime.sessions.set('idle', {
    lastAccessedAt: Date.now() - 1_000,
    session: { isStreaming: false, dispose() {} },
  })
  runtime.sessionHistoryCache.set('history', { size: 2_048 })

  const diagnostics = runtime.getRuntimeDiagnostics()

  assert.equal(diagnostics.workspaceCwd, directory)
  assert.equal(diagnostics.sessions.resident, 1)
  assert.equal(diagnostics.sessions.idle, 1)
  assert.equal(diagnostics.sessions.maxResident, 3)
  assert.equal(diagnostics.historyCache.entries, 1)
  assert.equal(diagnostics.historyCache.sourceBytes, 2_048)
  assert.equal(diagnostics.historyCache.estimatedBytes, 8_192)
  assert.equal(diagnostics.projectionCache.maxEntries, 8)
  assert.equal(diagnostics.projectionCache.maxEstimatedBytes, 24 * 1024 * 1024)
  assert.equal(diagnostics.projectionCache.transcripts.entries, 0)
  assert.ok(diagnostics.memory.rss > 0)
  assert.equal(JSON.stringify(diagnostics).includes('idle.jsonl'), false)
})
