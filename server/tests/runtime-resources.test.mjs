import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { defineTool, SessionManager } from '@earendil-works/pi-coding-agent'
import { Type } from 'typebox'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

test('main runtime keeps cold MCP tools dormant until explicitly requested while child resources remain available', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-runtime-resources-'))
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
  assert.match(value.session.agent.state.systemPrompt, /Application: Vesper/)
  assert.match(value.session.agent.state.systemPrompt, /Active model:/)
  assert.doesNotMatch(value.session.agent.state.systemPrompt, /You are Vesper/i)
  assert.equal(value.session.getActiveToolNames().includes('mcp_fixture_echo_12345678'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_list'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_manage'), false)
  assert.ok(value.session.getActiveToolNames().includes('read'))
  assert.ok(value.session.getActiveToolNames().includes('update_task_list'))
  const hotToolNames = value.session.getActiveToolNames()
  const hotSystemPrompt = value.session.agent.state.systemPrompt

  runtime.selectToolsForMessage(value, 'Use the MCP fixture echo tool for this task.')
  assert.ok(value.session.getActiveToolNames().includes('mcp_fixture_echo_12345678'))
  assert.ok(value.session.getActiveToolNames().includes('mcp_list'))
  assert.ok(value.session.getActiveToolNames().includes('mcp_manage'))
  assert.deepEqual(value.session.getActiveToolNames().slice(0, hotToolNames.length), hotToolNames)
  assert.equal(value.session.agent.state.systemPrompt, hotSystemPrompt)
  assert.match(value.session.getToolDefinition('mcp_manage').description, /Always use mcp_manage for MCP configuration/)
  assert.deepEqual(value.session.getToolDefinition('mcp_manage').promptGuidelines, [])

  runtime.selectToolsForMessage(value, 'Now update the local source file.')
  assert.equal(value.session.getActiveToolNames().includes('mcp_fixture_echo_12345678'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_list'), false)
  assert.equal(value.session.getActiveToolNames().includes('mcp_manage'), false)
  assert.equal(value.session.hasExtensionHandlers('tool_result'), false)
  assert.equal(value.session.hasExtensionHandlers('message_end'), false)

  value.isolatedContext = true
  value.blockedToolNames = ['memory_search', 'memory_remember']
  runtime.selectToolsForMessage(value, '搜索星忆并记住这项信息。')
  assert.equal(value.session.getActiveToolNames().includes('memory_search'), false)
  assert.equal(value.session.getActiveToolNames().includes('memory_remember'), false)

  const childLoader = await runtime.multiAgents.createResourceLoader({ cwd: directory, appendSystemPrompt: 'CHILD AGENT PROMPT' })
  assert.ok(childLoader.getSkills().skills.some((skill) => skill.name === 'runtime-skill'))
  assert.ok(childLoader.getAppendSystemPrompt().includes('CHILD AGENT PROMPT'))
})

test('background prompts apply their explicit execution mode before the Agent starts', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-background-prompt-'))
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
  const directory = await mkdtemp(join(tmpdir(), 'vesper-plugin-save-'))
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
  const directory = await mkdtemp(join(tmpdir(), 'vesper-resource-change-'))
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
