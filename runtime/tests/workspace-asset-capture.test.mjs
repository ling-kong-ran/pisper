import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  captureWorkspaceAssetBaseline,
  listChangedWorkspaceAssets,
  listNewWorkspaceAssets,
} from '../services/workspace-asset-capture.mjs'

async function fixture(t) {
  const workspace = await mkdtemp(join(tmpdir(), 'pisper-workspace-assets-'))
  t.after(() => rm(workspace, { recursive: true, force: true }))
  return workspace
}

test('workspace asset capture returns only new user files', async (t) => {
  const workspace = await fixture(t)
  await Promise.all([
    writeFile(join(workspace, 'existing.md'), '# Existing'),
    mkdir(join(workspace, 'node_modules', 'fixture'), { recursive: true }),
    mkdir(join(workspace, 'dist'), { recursive: true }),
  ])
  await writeFile(join(workspace, 'node_modules', 'fixture', 'package.js'), 'ignored')
  const baseline = await captureWorkspaceAssetBaseline(workspace)

  await Promise.all([
    writeFile(join(workspace, 'existing.md'), '# Updated'),
    writeFile(join(workspace, 'report.csv'), 'name,value\nPisper,1\n'),
    writeFile(join(workspace, 'demo.mp4'), Buffer.from('video fixture')),
    writeFile(join(workspace, 'package-lock.json'), '{}'),
    writeFile(join(workspace, 'dist', 'bundle.js'), 'ignored'),
  ])

  const assets = await listNewWorkspaceAssets(baseline)
  assert.deepEqual(
    assets.map((asset) => asset.path).sort(),
    [join(workspace, 'demo.mp4'), join(workspace, 'report.csv')].sort(),
  )
})

test('workspace asset capture ignores hidden, temporary, and generated directories', async (t) => {
  const workspace = await fixture(t)
  const baseline = await captureWorkspaceAssetBaseline(workspace)
  await Promise.all([
    mkdir(join(workspace, '.cache'), { recursive: true }),
    mkdir(join(workspace, 'target'), { recursive: true }),
    mkdir(join(workspace, 'notes'), { recursive: true }),
  ])
  await Promise.all([
    writeFile(join(workspace, '.cache', 'cache.txt'), 'ignored'),
    writeFile(join(workspace, 'target', 'program.log'), 'ignored'),
    writeFile(join(workspace, '.secret'), 'ignored'),
    writeFile(join(workspace, 'notes', 'meeting.md'), '# Meeting'),
  ])

  const assets = await listNewWorkspaceAssets(baseline)
  assert.deepEqual(
    assets.map((asset) => asset.path),
    [join(workspace, 'notes', 'meeting.md')],
  )
})

test('shell snapshots exclude runtime state and detect updates without dropping large batches', async (t) => {
  const workspace = await fixture(t)
  const dataDir = join(workspace, 'runtime-data')
  await mkdir(dataDir)
  await writeFile(join(workspace, 'report.txt'), 'before')
  const baseline = await captureWorkspaceAssetBaseline(workspace, { exclude: [dataDir] })
  await Promise.all([
    writeFile(join(dataDir, 'pisper-assets.json'), 'internal state'),
    writeFile(join(workspace, 'report.txt'), 'updated report'),
    ...Array.from({ length: 201 }, (_, i) => writeFile(join(workspace, `part-${i}.txt`), `${i}`)),
  ])
  const files = await listChangedWorkspaceAssets(baseline)
  assert.equal(files.length, 202)
  assert.ok(files.some((file) => file.path === join(workspace, 'report.txt')))
  assert.ok(files.every((file) => !file.path.startsWith(dataDir)))
})

// 集成回归：工具执行中产生的文件必须关联到所属会话并收录为资产。
async function turnRuntime(t, workspace, dataDir) {
  // dataDir 必须在工作区之外：生产环境它位于 ~/.pisper/agent；若放进工作区，
  // 会话存储与资产归档目录本身会被差集当成“新文件”误收。
  // 未走完整 init() 时手动建资产目录（生产由 init() 创建），否则归档 copyFile 会 ENOENT。
  await mkdir(join(dataDir, 'pisper-assets'), { recursive: true })
  const runtime = new AgentRuntimeService({ cwd: workspace, dataDir })
  // 与生产无关的旁路依赖：本测试只验证「差集收录」这条主线。
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.multiAgents.summaries = () => []
  return runtime
}

function turnSession(workspace, { failAfterWrite = false, skipWrite = false } = {}) {
  const tool = {
    name: 'write',
    async execute(_id, args) {
      await writeFile(args.path, args.content)
      if (failAfterWrite) throw new Error('model failed after writing')
      return { content: [{ type: 'text', text: 'written' }] }
    },
  }
  const session = {
    sessionId: 'session-1',
    model: { provider: 'local', id: 'pisper-test-model' },
    thinkingLevel: 'medium',
    isStreaming: false,
    messages: [],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName() {},
    setSessionName() {},
    subscribe: () => () => {},
    async prompt(text) {
      // 按 Pi 合同先调用权限钩，再执行其准备的工具；归档发生在工具边界。
      if (!skipWrite) {
        const args = { path: join(workspace, '标题-正文-注释.docx'), content: 'fake docx' }
        await session.agent.beforeToolCall?.({
          toolCall: { id: 'write-1', name: 'write' },
          args,
          context: { tools: [tool] },
        })
        await tool.execute('write-1', args)
      }
      session.messages.push({ role: 'user', content: text, timestamp: Date.now() })
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: '已生成文档' }],
        timestamp: Date.now(),
      })
    },
  }
  return session
}

async function runTurn(t, runtime, workspace, options = {}) {
  const session = turnSession(workspace, options)
  const value = { session, cwd: workspace, name: 'Workspace turn', baseToolNames: [] }
  runtime.sessions.set('session-1', value)
  const events = []
  await runtime.streamPrompt({
    sessionId: 'session-1',
    message: '生成 docx 文档',
    send: (event, data) => events.push({ event, data }),
  })
  return { runtime, events }
}

async function createTurnRuntime(t, workspace, dataDir) {
  const runtime = await turnRuntime(t, workspace, dataDir)
  t.after(async () => {
    await runtime.dispose?.().catch(() => {})
  })
  return runtime
}

test('turns that create new workspace files archive them as generated assets', async (t) => {
  const workspace = await fixture(t)
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-workspace-turn-data-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  await writeFile(join(workspace, 'existing.md'), '# Existing')

  const runtime = await createTurnRuntime(t, workspace, dataDir)
  const { events } = await runTurn(t, runtime, workspace)

  const assetEvents = events.filter((item) => item.event === 'generated_asset')
  assert.equal(assetEvents.length, 1)
  assert.equal(assetEvents[0].data.name, '标题-正文-注释.docx')
  const done = events.find((item) => item.event === 'done')
  assert.ok(done, 'missing done event')
  assert.equal(done.data.assets.length, 1)
  assert.equal(done.data.assets[0].name, '标题-正文-注释.docx')
  const assets = await runtime.listAssets({ sessionId: 'session-1' })
  assert.equal(assets.length, 1)
  assert.equal(assets[0].name, '标题-正文-注释.docx')
})

test('error turns still archive workspace files created before the failure', async (t) => {
  const workspace = await fixture(t)
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-workspace-turn-data-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))

  const runtime = await createTurnRuntime(t, workspace, dataDir)
  const { events } = await runTurn(t, runtime, workspace, { failAfterWrite: true })

  const error = events.find((item) => item.event === 'error')
  assert.ok(error, 'missing error event')
  assert.match(error.data.message, /model failed after writing/)
  assert.equal(error.data.assets.length, 1)
  assert.equal(error.data.assets[0].name, '标题-正文-注释.docx')
  const assets = await runtime.listAssets({ sessionId: 'session-1' })
  assert.equal(assets.length, 1)
})

test('a second turn does not re-archive files from earlier turns', async (t) => {
  const workspace = await fixture(t)
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-workspace-turn-data-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))

  // 生产环境是单个常驻 runtime 跨轮次复用：同一实例连跑两轮
  const runtime = await createTurnRuntime(t, workspace, dataDir)
  await runTurn(t, runtime, workspace)
  // 第二轮：文件已在基线中，不应再次产生 generated_asset 事件
  const { events } = await runTurn(t, runtime, workspace, { skipWrite: true })
  const assetEvents = events.filter((item) => item.event === 'generated_asset')
  assert.equal(assetEvents.length, 0)
  assert.equal((await runtime.listAssets({ sessionId: 'session-1' })).length, 1)
})
