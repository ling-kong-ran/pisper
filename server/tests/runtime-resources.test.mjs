import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { defineTool, SessionManager } from '@earendil-works/pi-coding-agent'
import { Type } from 'typebox'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

test('blank chat sessions stay lightweight until an Agent is first required', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-pending-session-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = {
    getGlobalSettings: () => ({ defaultProvider: 'openai', defaultModel: 'gpt-test' }),
  }
  runtime.saveSessionMeta = async () => {}
  runtime.listStoredSessions = async () => []
  let runtimeCreations = 0
  runtime.createSessionRuntime = async (manager, name) => {
    runtimeCreations += 1
    return { manager, name, created: 'runtime-created', modified: 'runtime-modified' }
  }

  const created = await runtime.createSession('Pending chat')
  assert.equal(runtimeCreations, 0)
  assert.equal(runtime.pendingSessions.has(created.id), true)
  assert.equal(created.model, 'openai/gpt-test')
  assert.equal((await runtime.listSessions())[0].name, 'Pending chat')

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
  await writeFile(join(directory, 'skills', 'runtime-skill', 'SKILL.md'), `---\nname: runtime-skill\ndescription: Verify runtime skill loading.\n---\n\nUse this runtime skill.\n`, 'utf8')

  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.init()
  runtime.mcp.createToolDefinitions = async () => [defineTool({
    name: 'mcp_fixture_echo_12345678',
    label: 'MCP fixture echo',
    description: 'Fixture MCP tool',
    parameters: Type.Object({ text: Type.String() }),
    async execute(_id, params) {
      return { content: [{ type: 'text', text: params.text }], details: {} }
    },
  })]

  const value = await runtime.createSessionRuntime(SessionManager.inMemory(directory))
  assert.ok(value.session.resourceLoader.getSkills().skills.some((skill) => skill.name === 'runtime-skill'))
  assert.ok(value.session.agent.state.systemPrompt.includes('runtime-skill'))
  assert.match(value.session.agent.state.systemPrompt, /Application: Pisper/)
  assert.match(value.session.agent.state.systemPrompt, /Active model:/)
  assert.doesNotMatch(value.session.agent.state.systemPrompt, /You are Pisper/i)
  assert.equal(value.session.getActiveToolNames().includes('mcp_fixture_echo_12345678'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_list'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_manage'), false)
  assert.ok(value.session.getActiveToolNames().includes('read'))
  assert.ok(value.session.getActiveToolNames().includes('update_task_list'))
  assert.ok(value.session.getActiveToolNames().includes('discover_tools'))
  assert.ok(value.session.getActiveToolNames().includes('generate_visual'))
  assert.match(value.session.agent.state.systemPrompt, /discover_tools/)
  assert.match(value.session.getToolDefinition('generate_visual').description, /mockup/)
  assert.deepEqual(value.session.getToolDefinition('generate_visual').parameters.properties.aspectRatio.enum, ['1:1', '16:9', '9:16', '4:3', '3:4'])
  assert.equal(value.session.getToolDefinition('generate_visual').parameters.properties.aspectRatio.anyOf, undefined)
  const hotToolNames = value.session.getActiveToolNames()
  const hotSystemPrompt = value.session.agent.state.systemPrompt

  await runtime.selectToolsForMessage(value, '先给我来个设计图，我看看样式是什么样的。')
  assert.deepEqual(value.requestedToolNames, [])
  assert.ok(value.session.getActiveToolNames().includes('generate_visual'))
  assert.deepEqual(value.promotedToolNames, [])
  assert.equal(value.session.agent.state.systemPrompt, hotSystemPrompt)

  const discovery = await value.session.getToolDefinition('discover_tools').execute(
    'discover-fixture',
    { query: 'MCP fixture echo', limit: 1 },
    new AbortController().signal,
  )
  assert.deepEqual(discovery.details.activated, ['mcp_fixture_echo_12345678'])
  assert.ok(value.session.getActiveToolNames().includes('mcp_fixture_echo_12345678'))

  // 删除正则路由后：selectToolsForMessage 不再按消息内容自动激活工具，已激活的 discover 工具保持
  await runtime.selectToolsForMessage(value, 'Use the MCP fixture echo tool for this task.')
  assert.ok(value.session.getActiveToolNames().includes('mcp_fixture_echo_12345678'))
  assert.equal(value.session.getActiveToolNames().includes('mcp_list'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_manage'), false)
  assert.deepEqual(value.session.getActiveToolNames().slice(0, hotToolNames.length), hotToolNames)
  assert.equal(value.session.agent.state.systemPrompt, hotSystemPrompt)
  assert.match(value.session.getToolDefinition('mcp_manage').description, /Always use mcp_manage for MCP configuration/)
  assert.deepEqual(value.session.getToolDefinition('mcp_manage').promptGuidelines, [])

  await runtime.selectToolsForMessage(value, 'Now update the local source file.')
  assert.equal(value.session.getActiveToolNames().includes('mcp_fixture_echo_12345678'), true)
  assert.equal(value.session.getActiveToolNames().includes('mcp_list'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_manage'), false)
  assert.deepEqual(value.promotedToolNames, ['mcp_fixture_echo_12345678'])
  assert.equal(runtime.sessionMeta[value.session.sessionId].promotedToolNames.includes('generate_visual'), false)
  assert.deepEqual(runtime.sessionMeta[value.session.sessionId].promotedToolNames, value.promotedToolNames)
  assert.equal(value.session.hasExtensionHandlers('tool_result'), false)
  assert.equal(value.session.hasExtensionHandlers('message_end'), false)

  value.isolatedContext = true
  value.blockedToolNames = ['memory_search', 'memory_remember']
  await runtime.selectToolsForMessage(value, '搜索星忆并记住这项信息。')
  assert.equal(value.session.getActiveToolNames().includes('memory_search'), false)
  assert.equal(value.session.getActiveToolNames().includes('memory_remember'), false)

  const childLoader = await runtime.multiAgents.createResourceLoader({ cwd: directory, appendSystemPrompt: 'CHILD AGENT PROMPT' })
  assert.ok(childLoader.getSkills().skills.some((skill) => skill.name === 'runtime-skill'))
  assert.ok(childLoader.getAppendSystemPrompt().includes('CHILD AGENT PROMPT'))
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
  runtime.setSessionCwd = async (_id, cwd) => { calls.push(['cwd', cwd]) }
  runtime.setSessionExecutionMode = async (_id, mode) => { calls.push(['executionMode', mode]) }
  runtime.streamPrompt = async ({ sessionId, isolatedContext, send }) => {
    calls.push(['stream', sessionId, isolatedContext])
    send('meta', { sessionId })
    send('text_delta', { delta: 'done' })
  }

  const result = await runtime.promptFromChannel({
    sessionId: '', message: 'run', cwd: directory, title: 'Scheduled task', executionMode: 'full-access', isolatedContext: true,
  })

  assert.deepEqual(calls.map(([type]) => type), ['cwd', 'executionMode', 'stream'])
  assert.equal(calls.at(-1)[2], true)
  assert.equal(runtime.sessions.get('scheduled-session').isolatedContext, true)
  assert.deepEqual(runtime.sessions.get('scheduled-session').blockedToolNames, ['memory_search', 'memory_remember'])
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
    session: { isStreaming: true, dispose: () => { streamingDisposed += 1 } },
  })
  runtime.sessions.set('idle', {
    runtimeVersion: 0,
    session: { isStreaming: false, dispose: () => { idleDisposed += 1 } },
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
    session: { isStreaming: true, dispose: () => { streamingDisposed += 1 } },
  })
  runtime.sessions.set('idle', {
    runtimeVersion: 0,
    session: { isStreaming: false, dispose: () => { idleDisposed += 1 } },
  })
  runtime.mcp.add = async () => ({ services: [] })

  await runtime.createMcpServer({ name: 'fixture' })

  assert.equal(runtime.sessionRuntimeVersion, 1)
  assert.equal(streamingDisposed, 0)
  assert.equal(idleDisposed, 1)
  assert.equal(runtime.sessions.has('streaming'), true)
  assert.equal(runtime.sessions.has('idle'), false)
})
