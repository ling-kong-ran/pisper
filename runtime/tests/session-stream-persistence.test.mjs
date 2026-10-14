import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

function deferred() {
  let resolve
  const promise = new Promise((settle) => {
    resolve = settle
  })
  return { promise, resolve }
}

for (const outcome of ['success', 'model-error', 'storage-error']) {
  test(`stream settlement drains terminal metadata persistence: ${outcome}`, async (t) => {
    const directory = await mkdtemp(join(tmpdir(), 'pisper-stream-persistence-'))
    const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
    const writeStarted = deferred()
    const releaseWrite = deferred()
    let run
    t.after(async () => {
      releaseWrite.resolve()
      await run
      await runtime.sessionMetaWrite.catch(() => {})
      await rm(directory, { recursive: true, force: true })
    })
    const listeners = new Set()
    const session = {
      sessionId: 'persistence-session',
      isStreaming: false,
      model: { provider: 'openai', id: 'gpt-5.4' },
      thinkingLevel: 'medium',
      messages: [{ role: 'user', content: 'Earlier', timestamp: 1 }],
      agent: { state: { systemPrompt: '' } },
      getActiveToolNames: () => [],
      setActiveToolsByName() {},
      subscribe(listener) {
        listeners.add(listener)
        return () => listeners.delete(listener)
      },
      async prompt() {
        const assistant = {
          role: 'assistant',
          content: [{ type: 'text', text: 'Completed turn' }],
          usage: { totalTokens: 10 },
          stopReason: 'stop',
        }
        for (const listener of listeners) {
          listener({ type: 'message_start', message: assistant })
          listener({ type: 'message_end', message: assistant })
        }
        session.messages.push(assistant)
        if (outcome === 'model-error') throw new Error('model failed')
      },
    }
    const value = { session, cwd: directory, name: 'Persistence', baseToolNames: [] }
    runtime.sessions.set(session.sessionId, value)
    runtime.getOrCreateSession = async () => value
    const saveSessionMeta = runtime.saveSessionMeta.bind(runtime)
    runtime.saveSessionMeta = async () => {
      writeStarted.resolve()
      await releaseWrite.promise
      if (outcome === 'storage-error') throw new Error('disk unavailable')
      return saveSessionMeta()
    }
    const events = []
    let settled = false
    run = runtime
      .streamPrompt({
        sessionId: session.sessionId,
        message: 'Save timing',
        send: (event, data) => events.push({ event, data }),
      })
      .finally(() => {
        settled = true
      })

    await writeStarted.promise
    // 用受控写盘屏障验证生命周期，不靠睡眠或文件系统调度概率复现竞态。
    await new Promise((resolve) => setImmediate(resolve))
    assert.equal(settled, false, 'the run must remain pending while metadata is being saved')
    const terminal = events.filter(({ event }) => event === 'done' || event === 'error')
    assert.equal(terminal.length, 1, 'the terminal event should not wait for storage')
    assert.equal(terminal[0].event, outcome === 'model-error' ? 'error' : 'done')
    assert.equal(listeners.size, 0, 'stream listeners should be detached before waiting')
    assert.equal(runtime.liveSessions.get(session.sessionId).streaming, false)

    releaseWrite.resolve()
    await run
    assert.equal(events.filter(({ event }) => event === 'done' || event === 'error').length, 1)
    if (outcome !== 'storage-error') {
      const meta = JSON.parse(await readFile(runtime.sessionMetaPath, 'utf8'))
      assert.equal(meta[session.sessionId].timing.requests, 1)
    }
  })
}
