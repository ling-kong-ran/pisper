import assert from 'node:assert/strict'
import test from 'node:test'
import {
  effectiveCompactionSettings,
  pisperCompactionExtension,
} from '../runtime/compaction-policy.mjs'
import { compact, estimateTokens, SessionManager } from '../runtime/pi-coding-agent.mjs'

const { prepareCompaction, shouldCompact } = await import(
  new URL('./core/compaction/index.js', import.meta.resolve('@earendil-works/pi-coding-agent')).href
)

const MODEL = {
  id: 'compaction-fixture',
  provider: 'fixture',
  contextWindow: 16_000,
  maxTokens: 4_096,
  reasoning: false,
}

function usage(output = 0) {
  return {
    input: 0,
    output,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: output,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  }
}

function appendToolRound(manager, index) {
  const toolCallId = `read-${index}`
  manager.appendMessage({
    role: 'assistant',
    content: [{ type: 'toolCall', id: toolCallId, name: 'read', arguments: { path: 'fixture' } }],
    usage: usage(),
    provider: MODEL.provider,
    model: MODEL.id,
    stopReason: 'toolUse',
    timestamp: Date.now(),
  })
  manager.appendMessage({
    role: 'toolResult',
    toolCallId,
    toolName: 'read',
    content: [{ type: 'text', text: 'word'.repeat(3_000) }],
    isError: false,
    timestamp: Date.now(),
  })
}

function longToolSession() {
  const manager = SessionManager.inMemory(process.cwd())
  manager.appendMessage({
    role: 'user',
    content: [{ type: 'text', text: 'Continue the current goal until the work is complete.' }],
    timestamp: Date.now(),
  })
  for (let index = 0; index < 5; index += 1) appendToolRound(manager, index)
  return manager
}

function contextTokens(manager) {
  return manager
    .buildSessionContext()
    .messages.reduce((sum, message) => sum + estimateTokens(message), 0)
}

function compactionHandler(streamFn) {
  let handler
  pisperCompactionExtension(
    {
      on(event, callback) {
        if (event === 'session_before_compact') handler = callback
      },
    },
    {
      compactSession: (...args) => {
        // 只替换摘要网络调用，切分、摘要拼接和上下文投影均执行锁定 SDK 的真实实现。
        args[7] = streamFn
        return compact(...args)
      },
    },
  )
  return async (manager, settings, signal) => {
    const preparation = prepareCompaction(manager.getBranch(), settings)
    assert.ok(preparation, 'crossing the threshold must permit useful compaction')
    const result = await handler(
      { preparation, signal },
      {
        model: MODEL,
        modelRegistry: {
          async getApiKeyAndHeaders() {
            return { ok: true, apiKey: 'fixture-key' }
          },
        },
      },
    )
    assert.ok(result?.compaction)
    const {
      summary,
      firstKeptEntryId,
      tokensBefore,
      details,
      usage: summaryUsage,
    } = result.compaction
    manager.appendCompaction(summary, firstKeptEntryId, tokensBefore, details, true, summaryUsage)
  }
}

test('small-window tool compaction leaves room for another tool round with paired results', async () => {
  const manager = longToolSession()
  const settings = effectiveCompactionSettings(
    { enabled: true, reserveTokens: 16_384, keepRecentTokens: 20_000 },
    MODEL.contextWindow,
    80,
  )
  const threshold = MODEL.contextWindow - settings.reserveTokens
  assert.equal(shouldCompact(contextTokens(manager), MODEL.contextWindow, settings), true)
  const summaryBudgets = []
  const performCompaction = compactionHandler(async (_model, _context, options) => {
    summaryBudgets.push(options.maxTokens)
    return {
      async result() {
        return {
          role: 'assistant',
          // 摘要用满实际请求上限，避免过短的 stub 隐藏压缩后仍超阈值的回归。
          content: [{ type: 'text', text: 's'.repeat(options.maxTokens * 4) }],
          stopReason: 'stop',
          usage: usage(options.maxTokens),
          timestamp: Date.now(),
        }
      },
    }
  })

  await performCompaction(manager, settings, new AbortController().signal)

  assert.ok(summaryBudgets.length > 0)
  assert.ok(contextTokens(manager) < threshold)
  const retained = manager.buildSessionContext().messages
  const pendingToolCalls = new Set()
  let retainedToolResults = 0
  for (const message of retained) {
    if (message.role === 'assistant') {
      for (const block of message.content) {
        if (block.type === 'toolCall') pendingToolCalls.add(block.id)
      }
    } else if (message.role === 'toolResult') {
      assert.ok(
        pendingToolCalls.delete(message.toolCallId),
        'a retained result needs its tool call',
      )
      retainedToolResults += 1
    }
  }
  assert.ok(retainedToolResults > 0, 'the latest tool output must remain available')
  assert.equal(pendingToolCalls.size, 0, 'retained tool calls must retain their results')

  appendToolRound(manager, 5)
  assert.equal(shouldCompact(contextTokens(manager), MODEL.contextWindow, settings), false)
  assert.equal(manager.getBranch().filter((entry) => entry.type === 'compaction').length, 1)
})

test('cancelling a real SDK summary preserves the previous tool context without a checkpoint', async () => {
  const manager = longToolSession()
  const before = manager.buildSessionContext()
  const leafBefore = manager.getLeafId()
  const settings = effectiveCompactionSettings({}, MODEL.contextWindow, 80)
  const controller = new AbortController()
  let started
  const summaryStarted = new Promise((resolve) => {
    started = resolve
  })
  const performCompaction = compactionHandler(async (_model, _context, options) => ({
    result() {
      return new Promise((_resolve, reject) => {
        const abort = () => reject(new DOMException('Summary cancelled', 'AbortError'))
        options.signal.addEventListener('abort', abort, { once: true })
        started()
      })
    },
  }))

  const pending = performCompaction(manager, settings, controller.signal)
  const rejected = assert.rejects(pending, { name: 'AbortError' })
  await summaryStarted
  controller.abort()
  await rejected

  assert.equal(manager.getLeafId(), leafBefore)
  assert.deepEqual(manager.buildSessionContext(), before)
  assert.equal(
    manager.getBranch().some((entry) => entry.type === 'compaction'),
    false,
  )
})
