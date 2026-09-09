import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createAgentSession, SessionManager, SettingsManager } from '../runtime/pi-coding-agent.mjs'
import { WorkspaceAssetTracker } from '../services/workspace-asset-tracker.mjs'

const { createExtensionRuntime } = await import(
  new URL('./core/extensions/loader.js', import.meta.resolve('@earendil-works/pi-coding-agent'))
    .href
)

// 使用真实 Pi 会话、并行工具批次和内置 write；只替代模型输出，避免外部凭据与网络。
test(
  'real Pi parallel write tools retain asset ownership without blocking batch preparation',
  { timeout: 15000 },
  async (t) => {
    const directory = await mkdtemp(join(tmpdir(), 'pisper-asset-sdk-'))
    t.after(() => rm(directory, { recursive: true, force: true }))
    const model = {
      id: 'asset-fixture',
      provider: 'fixture',
      api: 'openai-completions',
      name: 'Asset fixture',
      input: ['text'],
      reasoning: false,
      contextWindow: 128000,
      maxTokens: 4096,
    }
    const { session } = await createAgentSession({
      cwd: directory,
      agentDir: join(directory, '.agent'),
      model,
      modelRuntime: {},
      tools: ['write'],
      sessionManager: SessionManager.inMemory(directory),
      settingsManager: SettingsManager.inMemory({ compaction: { enabled: false } }),
      resourceLoader: {
        getExtensions: () => ({ extensions: [], errors: [], runtime: createExtensionRuntime() }),
        getSkills: () => ({ skills: [], diagnostics: [] }),
        getPrompts: () => ({ prompts: [], diagnostics: [] }),
        getThemes: () => ({ themes: [], diagnostics: [] }),
        getAgentsFiles: () => ({ agentsFiles: [] }),
        getSystemPrompt: () => 'Asset test',
        getAppendSystemPrompt: () => [],
        extendResources() {},
        async reload() {},
      },
    })
    t.after(() => session.dispose())
    const captured = []
    const tracker = new WorkspaceAssetTracker({
      dataDir: join(directory, '.agent'),
      archive: async (sessionId, path) => {
        captured.push({ sessionId, path, content: await readFile(path, 'utf8') })
        return { id: path, name: path }
      },
    })
    tracker.install(session, { sessionId: session.sessionId, cwd: directory })
    let calls = 0
    session.agent.getApiKey = () => 'fixture'
    session.agent.toolExecution = 'parallel'
    session.agent.streamFunction = () => {
      const first = calls++ === 0
      const message = {
        role: 'assistant',
        api: model.api,
        provider: model.provider,
        model: model.id,
        content: first
          ? ['report-a.txt', 'report-b.txt'].map((path, i) => ({
              type: 'toolCall',
              id: `write-${i}`,
              name: 'write',
              arguments: { path, content: path },
            }))
          : [{ type: 'text', text: 'done' }],
        stopReason: first ? 'toolUse' : 'stop',
        timestamp: Date.now(),
        usage: {
          input: 0,
          output: 0,
          cacheRead: 0,
          cacheWrite: 0,
          totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
      }
      return {
        async *[Symbol.asyncIterator]() {
          yield { type: 'done', reason: message.stopReason, message }
        },
        result: async () => message,
      }
    }
    await session.agent.prompt('Create two reports')
    assert.equal(calls, 2)
    assert.deepEqual(captured.map((file) => file.content).sort(), ['report-a.txt', 'report-b.txt'])
    assert.ok(captured.every((file) => file.sessionId === session.sessionId))
    assert.equal((await tracker.drain(session.sessionId)).length, 2)
  },
)
