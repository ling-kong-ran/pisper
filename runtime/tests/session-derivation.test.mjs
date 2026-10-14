import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { SessionManager } from '../runtime/pi-coding-agent.mjs'

function assistantMessage(text, stopReason = 'stop') {
  return {
    role: 'assistant',
    content: [{ type: 'text', text }],
    stopReason,
    timestamp: Date.now(),
  }
}

test('session derivation extracts a completed turn without changing its source session', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-derive-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = {
    getGlobalSettings: () => ({
      defaultProvider: 'openai',
      defaultModel: 'gpt-default',
      defaultThinkingLevel: 'medium',
    }),
  }

  const manager = SessionManager.create(directory, runtime.sessionDir)
  const sourceId = manager.getSessionId()
  manager.appendSessionInfo('Source session')
  manager.appendModelChange('openai', 'gpt-source')
  manager.appendThinkingLevelChange('high')
  const firstUserId = manager.appendMessage({
    role: 'user',
    content: 'First request',
    timestamp: Date.now(),
  })
  const firstBoundaryId = manager.appendMessage(assistantMessage('First response'))
  manager.appendMessage({ role: 'user', content: 'Second request', timestamp: Date.now() })
  const toolBoundaryId = manager.appendMessage({
    role: 'assistant',
    content: [{ type: 'toolCall', id: 'tool-1', name: 'read', arguments: { path: 'README.md' } }],
    stopReason: 'toolUse',
    timestamp: Date.now(),
  })
  manager.appendMessage({
    role: 'toolResult',
    toolCallId: 'tool-1',
    toolName: 'read',
    content: [{ type: 'text', text: 'result' }],
    isError: false,
    timestamp: Date.now(),
  })
  manager.appendMessage(assistantMessage('Second response'))
  const sourcePath = manager.getSessionFile()
  const sourceBefore = await readFile(sourcePath, 'utf8')
  runtime.sessionMeta[sourceId] = {
    name: 'Source session',
    cwd: directory,
    model: 'openai/gpt-source',
    executionMode: 'full-access',
    permissionMode: 'ignore',
  }
  await runtime.saveSessionMeta()

  await assert.rejects(
    runtime.deriveSession(sourceId, firstUserId, 'Invalid user boundary'),
    /已完成的 Agent 回复/,
  )
  await assert.rejects(
    runtime.deriveSession(sourceId, toolBoundaryId, 'Invalid tool boundary'),
    /已完成的 Agent 回复/,
  )

  const derived = await runtime.deriveSession(sourceId, firstBoundaryId, 'Derived session')
  const sourceAfter = await readFile(sourcePath, 'utf8')
  assert.equal(sourceAfter, sourceBefore)
  assert.notEqual(derived.id, sourceId)
  assert.equal(derived.name, 'Derived session')
  assert.equal(derived.cwd, directory)
  assert.equal(derived.model, 'openai/gpt-source')
  assert.equal(derived.thinkingLevel, 'high')
  assert.equal(derived.executionMode, 'full-access')
  assert.equal(derived.lineage.parentSessionId, sourceId)
  assert.equal(derived.lineage.sourceEntryId, firstBoundaryId)
  assert.equal(derived.lineage.sourceSessionName, 'Source session')

  const sourceTree = await runtime.getSessionTree(sourceId)
  assert.ok(sourceTree.lineage.childSessionIds.includes(derived.id))
  const derivedTree = await runtime.getSessionTree(derived.id)
  assert.equal(derivedTree.lineage.parentSessionId, sourceId)
  assert.equal(derivedTree.lineage.sourceEntryId, firstBoundaryId)

  const derivedInfo = await runtime.findSessionInfo(derived.id)
  const derivedManager = runtime.openStoredSession(derivedInfo.path)
  assert.equal(derivedManager.getHeader().parentSession, sourcePath)
  assert.deepEqual(
    derivedManager
      .getBranch()
      .filter((entry) => entry.type === 'message')
      .map((entry) => entry.message.content),
    ['First request', [{ type: 'text', text: 'First response' }]],
  )
  assert.equal(derivedManager.getSessionName(), 'Derived session')
  assert.equal(runtime.sessionMeta[derived.id].parentSessionId, sourceId)
  assert.equal(runtime.sessionMeta[derived.id].derivedFromEntryId, firstBoundaryId)

  const summaries = await runtime.listSessions()
  const sourceSummary = summaries.find((session) => session.id === sourceId)
  assert.deepEqual(sourceSummary.lineage.childSessionIds, [derived.id])

  // 源会话流式生成期间，仍可从已完成的历史节点衍生（历史节点已持久化）。
  runtime.liveSessions.set(sourceId, { streaming: true })
  const duringRun = await runtime.deriveSession(sourceId, firstBoundaryId, 'Running derivation')
  assert.notEqual(duringRun.id, sourceId)
  assert.equal(duringRun.lineage.sourceEntryId, firstBoundaryId)
  assert.equal(duringRun.lineage.parentSessionId, sourceId)
  runtime.liveSessions.delete(sourceId)
})
