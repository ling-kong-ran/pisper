// 工具预览图事件流集成测试：observe_ui 等工具的 tool_execution_end 携带内联图像时，
// 运行时异步归档资产并通过 tool_update 补发 previewImage；活动卡片状态同步更新，
// details.path 型工具（browser_automation）复用既有资产不重复写盘。
import assert from 'node:assert/strict'
import { mkdtemp, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

const PNG_1PX =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGBgAAAABQABh6FO1AAAAABJRU5ErkJggg=='

async function waitFor(predicate, { timeoutMs = 4_000, stepMs = 25 } = {}) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const value = await predicate()
    if (value) return value
    await new Promise((resolve) => setTimeout(resolve, stepMs))
  }
  return null
}

function createFakeSession({ sessionId, eventsByToolEnd = [] }) {
  const listeners = new Set()
  const session = {
    sessionId,
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'use the ui', timestamp: 1 }],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async prompt() {
      session.isStreaming = true
      for (const listener of listeners) listener({ type: 'agent_start' })
      for (const listener of listeners) listener({ type: 'turn_start' })
      for (const [index, toolEnd] of eventsByToolEnd.entries()) {
        for (const listener of listeners)
          listener({
            type: 'tool_execution_start',
            toolCallId: `tool-${index}`,
            toolName: toolEnd.toolName,
            args: {},
          })
        for (const listener of listeners)
          listener({
            type: 'tool_execution_end',
            toolCallId: `tool-${index}`,
            toolName: toolEnd.toolName,
            result: toolEnd.result,
            isError: false,
          })
      }
      const assistant = {
        role: 'assistant',
        content: [{ type: 'text', text: 'done' }],
        stopReason: 'stop',
        timestamp: 2,
      }
      session.messages.push(assistant)
      for (const listener of listeners) listener({ type: 'message_end', message: assistant })
      for (const listener of listeners)
        listener({ type: 'turn_end', message: assistant, toolResults: [] })
      for (const listener of listeners)
        listener({ type: 'agent_end', messages: [assistant], willRetry: false })
      for (const listener of listeners) listener({ type: 'agent_settled' })
      session.isStreaming = false
    },
  }
  return session
}

test('tool_execution_end 内联图像归档后补发 previewImage tool_update', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-tool-preview-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  const session = createFakeSession({
    sessionId: 'session-preview',
    eventsByToolEnd: [
      {
        toolName: 'observe_ui',
        result: {
          content: [
            { type: 'text', text: 'Outline (2 nodes, stateId s1)' },
            { type: 'image', data: PNG_1PX, mimeType: 'image/png' },
          ],
          details: {
            capture: { stateId: 's1', width: 800, height: 600 },
            target: {
              app: 'Weather',
              windowTitle: 'Weather — Cupertino',
              bundleId: 'com.apple.weather',
              windowId: 77,
            },
          },
        },
      },
    ],
  })
  const value = { session, cwd: directory, name: '预览会话', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: '看下窗口',
    send: (event, data) => events.push({ event, data }),
  })

  // 归档是异步的：等待补发的 tool_update 携带 previewImage。
  const previewUpdate = await waitFor(() =>
    events.find((item) => item.event === 'tool_update' && item.data.previewImage?.url),
  )
  assert.ok(previewUpdate, '应收到带 previewImage 的 tool_update')
  const { previewImage } = previewUpdate.data
  assert.match(previewImage.url, /^\/api\/assets\/[^/]+\/download\?inline=1$/)
  assert.equal(previewImage.mimeType, 'image/png')
  assert.equal(previewUpdate.data.name, 'observe_ui')
  // 目标窗口信息随同一事件透传：前端用它驱动实时镜像流。
  assert.deepEqual(previewUpdate.data.target, {
    app: 'Weather',
    windowTitle: 'Weather — Cupertino',
    bundleId: 'com.apple.weather',
    windowId: 77,
  })

  // 活动状态同步更新：tools / currentActivity 都能看到预览。
  // 注意：done 快照在运行收尾时生成，早于异步归档完成，因此不包含 previewImage；
  // 预览只随后续的 tool_update 与重新拉取的 live 快照出现。
  const live = await runtime.getSessionLive(session.sessionId)
  assert.equal(live.tools[0].previewImage.url, previewImage.url)

  // 资产实际落盘且通过 API 可查。
  const assetFiles = await readdir(join(directory, 'pisper-assets'))
  assert.equal(assetFiles.length, 1)
  const listed = await runtime.listAssets({ sessionId: session.sessionId })
  assert.equal(listed.length, 1)
  assert.equal(listed[0].kind, 'image')

  // 前端收到的载荷新字段保持纯 ASCII URL（TUI serde_json 安全）。
  assert.ok(!/[\u007f-\uffff]/.test(previewImage.url))
})

test('details.path 型工具复用既有资产生成 previewImage', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-tool-preview-path-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  // 预置一个已归档资产：模拟 browser_automation 截图已通过 recordGeneratedFile 落盘。
  const shotPath = join(directory, 'browser-shot.png')
  const { writeFile } = await import('node:fs/promises')
  await writeFile(shotPath, Buffer.from(PNG_1PX, 'base64'))
  await runtime.recordGeneratedFile('session-path', { name: '路径会话' }, shotPath)

  const session = createFakeSession({
    sessionId: 'session-path',
    eventsByToolEnd: [
      {
        toolName: 'browser_automation',
        result: {
          content: [{ type: 'text', text: 'screenshot saved' }],
          details: { path: shotPath },
        },
      },
    ],
  })
  const value = { session, cwd: directory, name: '路径会话', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: '截个图',
    send: (event, data) => events.push({ event, data }),
  })

  const previewUpdate = await waitFor(() =>
    events.find((item) => item.event === 'tool_update' && item.data.previewImage?.url),
  )
  assert.ok(previewUpdate, 'details.path 资产应生成 previewImage')
  // 复用既有资产：预览 URL 指向 recordGeneratedFile 归档的同一资产，且未新增条目。
  const listed = await runtime.listAssets({})
  assert.equal(listed.length, 1)
  const [asset] = listed
  assert.equal(
    previewUpdate.data.previewImage.url,
    `/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`,
  )
})

test('无图像的工具结果不会补发 previewImage', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-tool-preview-none-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  const session = createFakeSession({
    sessionId: 'session-none',
    eventsByToolEnd: [
      { toolName: 'read', result: { content: [{ type: 'text', text: 'file content' }] } },
    ],
  })
  const value = { session, cwd: directory, name: '普通会话', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: '读个文件',
    send: (event, data) => events.push({ event, data }),
  })
  await new Promise((resolve) => setTimeout(resolve, 150))
  assert.equal(
    events.some((item) => item.event === 'tool_update' && item.data.previewImage),
    false,
  )
  const assetDir = join(directory, 'pisper-assets')
  await rm(assetDir, { recursive: true, force: true }).catch(() => {})
})

test('语义模式（无图像）的 computer use 结果仍透传目标窗口', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-tool-preview-semantic-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  const session = createFakeSession({
    sessionId: 'session-semantic',
    eventsByToolEnd: [
      {
        toolName: 'act_ui',
        // 语义模式：结果只有文本大纲，没有内联图，也没有已归档文件。
        result: {
          content: [{ type: 'text', text: 'Outline (5 nodes, stateId s2)' }],
          details: {
            target: {
              app: 'Notes',
              windowTitle: 'Notes',
              bundleId: 'com.apple.Notes',
              windowId: 88,
            },
          },
        },
      },
    ],
  })
  const value = { session, cwd: directory, name: '语义会话', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: '操作备忘录',
    send: (event, data) => events.push({ event, data }),
  })

  // 无图也应收到仅带 target 的 tool_update：实时镜像流需要它保活。
  const targetUpdate = await waitFor(() =>
    events.find((item) => item.event === 'tool_update' && item.data.target),
  )
  assert.ok(targetUpdate, '语义模式应透传目标窗口')
  assert.equal(targetUpdate.data.target.windowId, 88)
  assert.equal(targetUpdate.data.name, 'act_ui')
  assert.equal(targetUpdate.data.previewImage, undefined)
  // 活动状态也带上了目标信息。
  const live = await runtime.getSessionLive(session.sessionId)
  assert.equal(live.tools[0].target.windowId, 88)
})
