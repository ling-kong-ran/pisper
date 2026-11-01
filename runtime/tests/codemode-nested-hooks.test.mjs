// codemode 嵌套工具调用测试：真实 Pi 会话 + 真实 codemode 沙箱，只替代模型输出。
// 脚本里调用的工具必须经过 agent.beforeToolCall 钩子链（审批、文件快照），不能绕过。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  createAgentSession,
  createCodemodeExtension,
  SessionManager,
  SettingsManager,
} from '../runtime/pi-coding-agent.mjs'
import { SessionFileChangesService } from '../services/session-file-changes.mjs'

const packageUrl = import.meta.resolve('@earendil-works/pi-coding-agent')
const { createExtensionRuntime, loadExtensionFromFactory } = await import(
  new URL('./core/extensions/loader.js', packageUrl).href
)
const { createEventBus } = await import(new URL('./core/event-bus.js', packageUrl).href)

function assistant(model, content, stopReason) {
  return {
    role: 'assistant',
    api: model.api,
    provider: model.provider,
    model: model.id,
    content,
    stopReason,
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
}

test(
  'codemode nested tool calls pass through Pisper beforeToolCall hooks',
  { timeout: 30000 },
  async (t) => {
    const directory = await mkdtemp(join(tmpdir(), 'pisper-codemode-hooks-'))
    t.after(() => rm(directory, { recursive: true, force: true }))
    const model = {
      id: 'codemode-fixture',
      provider: 'fixture',
      api: 'openai-completions',
      name: 'Codemode fixture',
      input: ['text'],
      reasoning: false,
      contextWindow: 128000,
      maxTokens: 4096,
    }
    const runtime = createExtensionRuntime()
    const codemode = await loadExtensionFromFactory(
      createCodemodeExtension(),
      directory,
      createEventBus(),
      runtime,
      '<codemode>',
    )
    const { session } = await createAgentSession({
      cwd: directory,
      agentDir: join(directory, '.agent'),
      model,
      modelRuntime: {},
      tools: ['write', 'codemode'],
      sessionManager: SessionManager.inMemory(directory),
      settingsManager: SettingsManager.inMemory({ compaction: { enabled: false } }),
      resourceLoader: {
        getExtensions: () => ({ extensions: [codemode], errors: [], runtime }),
        getSkills: () => ({ skills: [], diagnostics: [] }),
        getPrompts: () => ({ prompts: [], diagnostics: [] }),
        getThemes: () => ({ themes: [], diagnostics: [] }),
        getAgentsFiles: () => ({ agentsFiles: [] }),
        getSystemPrompt: () => 'Codemode test',
        getAppendSystemPrompt: () => [],
        extendResources() {},
        async reload() {},
      },
    })
    t.after(() => session.dispose())
    assert.ok(session.getActiveToolNames().includes('codemode'))

    // 文件快照与一个模拟审批共同组成钩子链；审批拒绝 blocked.txt。
    const changes = new SessionFileChangesService({
      dataDir: join(directory, '.agent'),
      warn: () => {},
    })
    await changes.markSessionTracked(session.sessionId, directory)
    changes.install(session, { sessionId: session.sessionId, cwd: directory })
    const seen = []
    const upstream = session.agent.beforeToolCall
    session.agent.beforeToolCall = async (context, signal) => {
      seen.push({ name: context.toolCall.name, path: context.args?.path })
      if (context.args?.path === 'blocked.txt') return { block: true, reason: 'Denied by test.' }
      return upstream?.(context, signal)
    }

    let calls = 0
    let toolResult = ''
    session.agent.getApiKey = () => 'fixture'
    session.agent.streamFunction = (_model, context) => {
      const first = calls++ === 0
      if (!first) {
        const result = context.messages.findLast((message) => message.role === 'toolResult')
        toolResult = (result?.content || []).map((item) => item.text || '').join('\n')
      }
      const message = first
        ? assistant(
            model,
            [
              {
                type: 'toolCall',
                id: 'script-1',
                name: 'codemode',
                arguments: {
                  code: [
                    "await tools.write({ path: 'allowed.txt', content: 'from script' })",
                    "try { await tools.write({ path: 'blocked.txt', content: 'nope' }) } catch (error) { text('blocked: ' + error.message) }",
                  ].join('\n'),
                },
              },
            ],
            'toolUse',
          )
        : assistant(model, [{ type: 'text', text: 'done' }], 'stop')
      return {
        async *[Symbol.asyncIterator]() {
          yield { type: 'done', reason: message.stopReason, message }
        },
        result: async () => message,
      }
    }

    await session.agent.prompt('Write files from a script')
    assert.equal(calls, 2)
    assert.deepEqual(
      seen.map((call) => `${call.name}:${call.path ?? ''}`),
      ['codemode:', 'write:allowed.txt', 'write:blocked.txt'],
    )
    assert.equal(await readFile(join(directory, 'allowed.txt'), 'utf8'), 'from script')
    await assert.rejects(stat(join(directory, 'blocked.txt')))
    assert.match(toolResult, /blocked: .*Denied by test/)
    const list = await changes.list(session.sessionId, directory)
    assert.deepEqual(
      list.files.map((file) => [file.path, file.status]),
      [['allowed.txt', 'created']],
    )
  },
)
