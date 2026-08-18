import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { SessionManager } from '@earendil-works/pi-coding-agent'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  TREE_NAVIGATION_CUSTOM_TYPE,
  appendTreePosition,
  projectSessionTree,
} from '../runtime/session-tree.mjs'

function assistantMessage(text, timestamp = Date.now()) {
  return {
    role: 'assistant',
    content: [{ type: 'text', text }],
    provider: 'openai',
    model: 'gpt-test',
    api: 'openai-responses',
    usage: {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    },
    stopReason: 'stop',
    timestamp,
  }
}

function appendConversation(manager) {
  const firstUser = manager.appendMessage({
    role: 'user',
    content: 'First question',
    timestamp: Date.now(),
  })
  const firstAssistant = manager.appendMessage(assistantMessage('First answer'))
  const originalUser = manager.appendMessage({
    role: 'user',
    content: 'Original follow-up',
    timestamp: Date.now(),
  })
  const originalAssistant = manager.appendMessage(assistantMessage('Original branch answer'))
  manager.branch(firstAssistant)
  const alternateUser = manager.appendMessage({
    role: 'user',
    content: 'Alternate follow-up',
    timestamp: Date.now(),
  })
  const alternateAssistant = manager.appendMessage(assistantMessage('Alternate branch answer'))
  return {
    firstUser,
    firstAssistant,
    originalUser,
    originalAssistant,
    alternateUser,
    alternateAssistant,
  }
}

test('session tree projection exposes stable branches without tool arguments or custom data', () => {
  const manager = SessionManager.inMemory(process.cwd())
  const entries = appendConversation(manager)
  const toolCallEntry = manager.appendMessage({
    role: 'assistant',
    content: [
      {
        type: 'toolCall',
        id: 'call-secret',
        name: 'read',
        arguments: { path: 'C:/private/secret.txt', token: 'secret-token' },
      },
    ],
    provider: 'openai',
    model: 'gpt-test',
    api: 'openai-responses',
    usage: assistantMessage('').usage,
    stopReason: 'toolUse',
    timestamp: Date.now(),
  })
  manager.appendCustomEntry('fixture.private-state', { token: 'private-custom-data' })
  manager.appendLabelChange(entries.firstAssistant, 'Decision point')

  const tree = projectSessionTree(manager)
  const nodes = tree.nodes
  assert.equal(tree.branchCount, 1)
  assert.equal(nodes.find((node) => node.id === entries.firstAssistant)?.branchPoint, true)
  assert.equal(nodes.find((node) => node.id === entries.originalAssistant)?.active, false)
  assert.equal(nodes.find((node) => node.id === entries.alternateAssistant)?.active, true)
  assert.equal(nodes.find((node) => node.id === entries.firstAssistant)?.label, 'Decision point')
  assert.equal(nodes.find((node) => node.id === toolCallEntry)?.kind, 'tool-call')
  assert.match(JSON.stringify(tree), /read/)
  assert.doesNotMatch(JSON.stringify(tree), /secret-token|private-custom-data|secret\.txt/)
})

test('session tree projection stays stack-safe for deep linear sessions', () => {
  const depth = 7_000
  const entries = []
  let root = null
  let cursor = null
  for (let index = 0; index < depth; index += 1) {
    const entry = {
      id: `entry-${index}`,
      parentId: index > 0 ? `entry-${index - 1}` : null,
      type: 'custom',
      customType: 'fixture.deep-entry',
      timestamp: new Date(index).toISOString(),
    }
    const node = { entry, children: [], label: undefined }
    if (cursor) cursor.children.push(node)
    else root = node
    cursor = node
    entries.push(entry)
  }
  const tree = projectSessionTree({
    getBranch: () => entries,
    getLeafId: () => entries.at(-1).id,
    getSessionId: () => 'deep-session',
    getTree: () => [root],
  })
  assert.equal(tree.nodeCount, depth)
  assert.equal(tree.nodes.length, depth)
  assert.equal(tree.nodes.at(-1).leaf, true)
  assert.doesNotThrow(() => JSON.stringify(tree))
})

test('session tree position entries persist a selected Pi leaf without entering model context', () => {
  const manager = SessionManager.inMemory(process.cwd())
  const entries = appendConversation(manager)
  manager.branch(entries.originalAssistant)
  const contextBefore = manager.buildSessionContext().messages
  const markerId = appendTreePosition(manager, entries.originalAssistant)
  const marker = manager.getEntry(markerId)

  assert.equal(manager.getLeafId(), markerId)
  assert.equal(marker.type, 'custom')
  assert.equal(marker.customType, TREE_NAVIGATION_CUSTOM_TYPE)
  assert.deepEqual(manager.buildSessionContext().messages, contextBefore)
})

test('session tree position reuses an unused marker and allows a new one after a user message', () => {
  const manager = SessionManager.inMemory(process.cwd())
  const userId = manager.appendMessage({
    role: 'user',
    content: 'Question',
    timestamp: Date.now(),
  })
  const assistantId = manager.appendMessage(assistantMessage('Answer'))
  manager.branch(assistantId)

  const firstMarkerId = appendTreePosition(manager, assistantId)
  const reusedMarkerId = appendTreePosition(manager, assistantId)
  assert.equal(reusedMarkerId, firstMarkerId)
  assert.equal(manager.getEntries().length, 3)

  manager.branch(assistantId)
  manager.appendMessage({
    role: 'user',
    content: 'Follow-up',
    timestamp: Date.now(),
  })
  manager.branch(assistantId)
  const secondMarkerId = appendTreePosition(manager, assistantId)
  assert.notEqual(secondMarkerId, firstMarkerId)
  assert.notEqual(secondMarkerId, userId)
  assert.equal(manager.getEntries().length, 5)
})

test('session tree projection hides consumed and duplicate pending markers', () => {
  const manager = SessionManager.inMemory(process.cwd())
  manager.appendMessage({ role: 'user', content: 'Question', timestamp: Date.now() })
  const assistantId = manager.appendMessage(assistantMessage('Answer'))
  manager.branch(assistantId)
  const consumedMarkerId = manager.appendCustomEntry(TREE_NAVIGATION_CUSTOM_TYPE, {
    targetId: assistantId,
  })
  manager.branch(assistantId)
  const followUpId = manager.appendMessage({
    role: 'user',
    content: 'Follow-up',
    timestamp: Date.now(),
  })
  manager.branch(assistantId)
  const pendingMarkerId = manager.appendCustomEntry(TREE_NAVIGATION_CUSTOM_TYPE, {
    targetId: assistantId,
  })
  manager.branch(assistantId)
  const duplicatePendingMarkerId = manager.appendCustomEntry(TREE_NAVIGATION_CUSTOM_TYPE, {
    targetId: assistantId,
  })
  manager.branch(pendingMarkerId)

  const tree = projectSessionTree(manager)
  const positionIds = tree.nodes.filter((node) => node.kind === 'position').map((node) => node.id)
  assert.deepEqual(positionIds, [pendingMarkerId])
  assert.equal(
    tree.nodes.some((node) => node.id === consumedMarkerId),
    false,
  )
  assert.equal(
    tree.nodes.some((node) => node.id === duplicatePendingMarkerId),
    false,
  )
  assert.equal(
    tree.nodes.some((node) => node.id === followUpId),
    true,
  )
  assert.equal(tree.nodeCount, 4)
})

test('runtime navigation uses AgentSession tree semantics and survives a cold reload', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-tree-'))
  let runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime?.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  const created = await runtime.createSession('Tree fixture', directory)
  const manager = runtime.pendingSessions.get(created.id).manager
  manager.appendModelChange('openai', 'gpt-test')
  const entries = appendConversation(manager)

  const before = await runtime.getSessionTree(created.id)
  assert.equal(before.branchCount, 1)
  assert.equal(before.leafId, entries.alternateAssistant)

  // Match a persisted labeled chat: runtime materialization is outside the navigation measurement.
  await runtime.getOrCreateSession(created.id)
  const listStoredSessions = runtime.listStoredSessions.bind(runtime)
  let catalogRefreshes = 0
  runtime.listStoredSessions = (options) => {
    if (options?.refresh) catalogRefreshes += 1
    return listStoredSessions(options)
  }
  const compactNavigation = await runtime.navigateSessionTree(
    created.id,
    entries.originalAssistant,
    {
      summarize: false,
      includeTree: false,
    },
  )
  assert.deepEqual(compactNavigation, { cancelled: false, editorText: null })
  assert.equal(catalogRefreshes, 0)

  const firstTree = await runtime.getSessionTree(created.id)
  const navigated = await runtime.navigateSessionTree(created.id, entries.originalAssistant, {
    summarize: false,
  })
  assert.equal(navigated.cancelled, false)
  assert.equal(navigated.editorText, null)
  assert.notEqual(navigated.leafId, entries.originalAssistant)
  assert.equal(navigated.leafId, firstTree.leafId)
  assert.equal(navigated.nodeCount, firstTree.nodeCount)
  assert.equal(navigated.nodes.find((node) => node.id === navigated.leafId)?.kind, 'position')
  assert.equal(navigated.nodes.find((node) => node.id === entries.originalAssistant)?.active, true)
  assert.equal(
    navigated.nodes.find((node) => node.id === entries.alternateAssistant)?.active,
    false,
  )
  assert.equal(catalogRefreshes, 0)

  await runtime.dispose()
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.init()
  const reloaded = await runtime.getSessionTree(created.id)
  assert.equal(reloaded.leafId, navigated.leafId)
  assert.equal(reloaded.nodes.find((node) => node.id === entries.originalAssistant)?.active, true)

  const fromUser = await runtime.navigateSessionTree(created.id, entries.originalUser, {
    summarize: false,
  })
  assert.equal(fromUser.editorText, 'Original follow-up')
  assert.equal(fromUser.nodes.find((node) => node.id === entries.firstAssistant)?.active, true)
  assert.equal(fromUser.nodes.find((node) => node.id === entries.originalUser)?.active, false)

  const labeled = await runtime.setSessionTreeLabel(
    created.id,
    entries.firstAssistant,
    'Resume here',
  )
  assert.equal(
    labeled.nodes.find((node) => node.id === entries.firstAssistant)?.label,
    'Resume here',
  )
  await runtime.setSessionTreeLabel(created.id, entries.firstUser, 'Resume user message')
  const labelMatches = await runtime.searchSessionTreeLabels('resume')
  assert.equal(labelMatches.length, 1)
  const [resumeMatch] = labelMatches
  assert.equal(resumeMatch.sessionId, created.id)
  assert.equal(resumeMatch.sessionName, 'Tree fixture')
  assert.equal(resumeMatch.entryId, entries.firstAssistant)
  assert.equal(resumeMatch.label, 'Resume here')
  assert.equal(resumeMatch.summary, '')
  assert.equal(resumeMatch.active, true)
  // 目录时间戳与 createSession 返回值可能有毫秒级漂移，只做容差断言。
  assert.ok(
    Math.abs(Date.parse(resumeMatch.sessionCreated) - Date.parse(created.created)) < 5000,
    'sessionCreated should match the catalog within tolerance',
  )
  assert.ok(Date.parse(resumeMatch.nodeTimestamp) > 0)
  assert.deepEqual(await runtime.searchSessionTreeLabels('missing'), [])
  // 文件 (mtime, size) 变化使扫描缓存失效：绕过 setSessionTreeLabel 直接追加
  // label 条目后，搜索仍应读到最新内容；user 消息上的标签不计入结果。
  const labelManager = runtime.sessions.get(created.id).session.sessionManager
  labelManager.appendLabelChange(entries.firstUser, 'Resume user checkpoint')
  assert.deepEqual(await runtime.searchSessionTreeLabels('user checkpoint'), [])
  labelManager.appendLabelChange(entries.alternateAssistant, 'Resume alternate')
  const rescanned = await runtime.searchSessionTreeLabels('alternate')
  assert.equal(rescanned.length, 1)
  assert.equal(rescanned[0].entryId, entries.alternateAssistant)
  assert.equal(rescanned[0].label, 'Resume alternate')
  // 缓存命中路径返回一致结果
  assert.deepEqual(await runtime.searchSessionTreeLabels('alternate'), rescanned)
  // 删除：空标签写入墓碑后，搜索与全量列表都不再返回该标签。
  await runtime.setSessionTreeLabel(created.id, entries.alternateAssistant, '')
  assert.deepEqual(await runtime.searchSessionTreeLabels('alternate'), [])
  const allLabels = await runtime.searchSessionTreeLabels('')
  assert.equal(allLabels.length, 1)
  assert.equal(allLabels[0].entryId, entries.firstAssistant)
  assert.equal(allLabels[0].label, 'Resume here')
  const active = runtime.sessions.get(created.id)
  const entryCount = active.session.sessionManager.getEntries().length
  await runtime.setSessionTreeLabel(created.id, entries.firstAssistant, 'Resume here')
  assert.equal(active.session.sessionManager.getEntries().length, entryCount)
  active.runActive = true
  await assert.rejects(
    runtime.navigateSessionTree(created.id, entries.alternateAssistant),
    /正在运行/,
  )
  active.runActive = false
  await assert.rejects(runtime.navigateSessionTree(created.id, 'missing-entry'), /节点不存在/)
  // 持久化索引：重启后的首次搜索无需重扫会话文件，直接命中磁盘索引。
  const indexPath = join(directory, 'pisper-session-label-index.json')
  const indexRaw = JSON.parse(await readFile(indexPath, 'utf8'))
  assert.equal(indexRaw.version, 1)
  assert.ok(Object.keys(indexRaw.files).length >= 1)
  await runtime.dispose()
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.init()
  const fromIndex = await runtime.searchSessionTreeLabels('')
  assert.equal(fromIndex.length, 1)
  assert.equal(fromIndex[0].entryId, entries.firstAssistant)
  assert.equal(fromIndex[0].label, 'Resume here')
})
