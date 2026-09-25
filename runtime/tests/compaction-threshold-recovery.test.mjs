import assert from 'node:assert/strict'
import test from 'node:test'
import { compact, SessionManager } from '../runtime/pi-coding-agent.mjs'
import {
  effectiveCompactionSettings,
  pisperCompactionExtension,
} from '../runtime/compaction-policy.mjs'

const engineEntry = import.meta.resolve('@earendil-works/pi-coding-agent')
const { estimateTokens, prepareCompaction, shouldCompact } = await import(
  new URL('./core/compaction/compaction.js', engineEntry)
)

const usage = {
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheWrite: 0,
  totalTokens: 0,
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
}

function projectedTokens(manager) {
  return manager.buildSessionContext().messages.reduce((total, message) => {
    return total + estimateTokens(message)
  }, 0)
}

function appendToolTurn(manager, index, resultTokens = 1_000) {
  manager.appendMessage({
    role: 'assistant',
    content: [
      { type: 'toolCall', id: `read-${index}`, name: 'read', arguments: { path: 'fixture.txt' } },
    ],
    api: 'fixture',
    provider: 'fixture',
    model: 'fixture',
    usage,
    stopReason: 'toolUse',
    timestamp: index * 2,
  })
  manager.appendMessage({
    role: 'toolResult',
    toolCallId: `read-${index}`,
    toolName: 'read',
    content: [{ type: 'text', text: 'data'.repeat(resultTokens) }],
    isError: false,
    timestamp: index * 2 + 1,
  })
}

function assertPairedToolResults(messages) {
  const calls = new Set()
  const results = new Set()
  for (const message of messages) {
    if (message.role === 'assistant') {
      for (const block of message.content) {
        if (block.type === 'toolCall') calls.add(block.id)
      }
    } else if (message.role === 'toolResult') {
      assert.ok(calls.has(message.toolCallId), 'retained tool results must retain their calls')
      results.add(message.toolCallId)
    }
  }
  assert.deepEqual(results, calls, 'compaction must preserve complete tool call/result pairs')
}

for (const { contextWindow, thresholdPercent } of [
  { contextWindow: 32_000, thresholdPercent: 50 },
  { contextWindow: 128_000, thresholdPercent: 10 },
  { contextWindow: 8_000, thresholdPercent: 80 },
]) {
  test(`compaction leaves room for a Goal continuation at ${contextWindow} tokens and ${thresholdPercent}%`, async () => {
    const manager = SessionManager.inMemory()
    manager.appendMessage({
      role: 'user',
      content: 'Complete the fixture task, retaining verified progress.',
      timestamp: 0,
    })
    // 长任务或切换到较小窗口模型时，已有历史可能远超新的触发线。
    for (let index = 1; index <= 32; index += 1) appendToolTurn(manager, index)
    const settings = effectiveCompactionSettings({}, contextWindow, thresholdPercent)
    const triggerTokens = contextWindow - settings.reserveTokens
    assert.ok(shouldCompact(projectedTokens(manager), contextWindow, settings))
    const preparation = prepareCompaction(manager.getBranch(), settings)
    assert.ok(preparation, 'the existing history must have a compactable prefix')

    const branchBefore = manager.getBranch()
    const retainedIndex = branchBefore.findIndex(
      (entry) => entry.id === preparation.firstKeptEntryId,
    )
    assert.ok(retainedIndex > 0)
    const retainedMessages = structuredClone(
      branchBefore.slice(retainedIndex).flatMap((entry) => {
        return entry.type === 'message' ? [entry.message] : []
      }),
    )

    let beforeCompact
    pisperCompactionExtension(
      {
        on(event, handler) {
          if (event === 'session_before_compact') beforeCompact = handler
        },
      },
      {
        // 摘要替身遵循生产预算；真实 SDK 仍负责裁切和压缩后的上下文投影。
        compactSession: async (prepared, model) => ({
          summary: 'note'.repeat(
            Math.max(1, Math.floor(Math.min(prepared.settings.reserveTokens / 2, model.maxTokens))),
          ),
          firstKeptEntryId: prepared.firstKeptEntryId,
          tokensBefore: prepared.tokensBefore,
          usage,
          details: {},
        }),
      },
    )
    const { compaction } = await beforeCompact(
      { preparation, reason: 'threshold', signal: new AbortController().signal },
      {
        model: { id: 'fixture', provider: 'fixture', contextWindow, maxTokens: 8_192 },
        modelRegistry: {
          getApiKeyAndHeaders: async () => ({ ok: true, apiKey: 'fixture' }),
        },
      },
    )
    manager.appendCompaction(
      compaction.summary,
      compaction.firstKeptEntryId,
      compaction.tokensBefore,
      compaction.details,
      true,
      compaction.usage,
    )

    const projected = manager.buildSessionContext().messages
    assert.deepEqual(projected.slice(-retainedMessages.length), retainedMessages)
    assertPairedToolResults(projected)
    const afterTokens = projectedTokens(manager)
    assert.ok(
      afterTokens < triggerTokens,
      `compaction retained ${afterTokens} tokens, above its ${triggerTokens}-token trigger`,
    )

    manager.appendMessage({
      role: 'user',
      content: '[Pisper internal goal continuation]\nContinue the next concrete action.',
      timestamp: 100,
    })
    appendToolTurn(manager, 100, 200)
    assert.equal(
      shouldCompact(projectedTokens(manager), contextWindow, settings),
      false,
      'a short Goal continuation must not immediately trigger another compaction',
    )
    assertPairedToolResults(manager.buildSessionContext().messages)
  })
}

test('switching to a smaller model recompresses the previous summary with the current turn prefix', async () => {
  const manager = SessionManager.inMemory()
  manager.appendMessage({ role: 'user', content: 'Complete the earlier task.', timestamp: 0 })
  const currentTurnId = manager.appendMessage({
    role: 'user',
    content: 'Continue the remaining fixture work.',
    timestamp: 1,
  })
  // 旧摘要和历史能装进新窗口，但直接保留旧摘要会挤掉压缩后继续工作的空间。
  const previousSummary = `Preserve the earlier verified decision.\n${'past'.repeat(12_250)}`
  manager.appendCompaction(previousSummary, currentTurnId, 100_000)
  for (let index = 1; index <= 14; index += 1) appendToolTurn(manager, index)

  const model = {
    id: 'fixture',
    provider: 'fixture',
    contextWindow: 32_000,
    maxTokens: 4_096,
    reasoning: false,
  }
  const settings = effectiveCompactionSettings({}, model.contextWindow, 50)
  assert.ok(projectedTokens(manager) < model.contextWindow)
  assert.ok(shouldCompact(projectedTokens(manager), model.contextWindow, settings))
  const preparation = prepareCompaction(manager.getBranch(), settings)
  assert.ok(preparation)
  assert.equal(preparation.isSplitTurn, true)
  assert.equal(preparation.messagesToSummarize.length, 0)
  assert.ok(preparation.turnPrefixMessages.length > 0)
  assert.equal(preparation.previousSummary, previousSummary)

  const branchBefore = manager.getBranch()
  const retainedIndex = branchBefore.findIndex((entry) => entry.id === preparation.firstKeptEntryId)
  const retainedMessages = structuredClone(
    branchBefore.slice(retainedIndex).flatMap((entry) => {
      return entry.type === 'message' ? [entry.message] : []
    }),
  )
  let beforeCompact
  const requests = []
  pisperCompactionExtension(
    {
      on(event, handler) {
        if (event === 'session_before_compact') beforeCompact = handler
      },
    },
    {
      compactSession: (...args) => {
        // 只替换网络响应，让 SDK 真实执行旧摘要更新或分段拼接的选择。
        args[7] = async (_model, context, options) => {
          const requestTokens = context.messages.reduce(
            (total, message) => total + estimateTokens(message),
            Math.ceil((context.systemPrompt?.length || 0) / 4),
          )
          assert.ok(
            requestTokens + options.maxTokens < model.contextWindow,
            'the summarization fixture must fit the new model window including its output',
          )
          requests.push(context)
          return {
            result: async () => ({
              role: 'assistant',
              content: [{ type: 'text', text: 's'.repeat(options.maxTokens * 4) }],
              usage: { ...usage, output: options.maxTokens, totalTokens: options.maxTokens },
              stopReason: 'stop',
              timestamp: 100,
            }),
          }
        }
        return compact(...args)
      },
    },
  )
  const { compaction } = await beforeCompact(
    { preparation, reason: 'threshold', signal: new AbortController().signal },
    {
      model,
      modelRegistry: { getApiKeyAndHeaders: async () => ({ ok: true, apiKey: 'fixture' }) },
    },
  )
  manager.appendCompaction(
    compaction.summary,
    compaction.firstKeptEntryId,
    compaction.tokensBefore,
    compaction.details,
    true,
    compaction.usage,
  )

  const afterTokens = projectedTokens(manager)
  const triggerTokens = model.contextWindow - settings.reserveTokens
  assert.ok(
    afterTokens < triggerTokens,
    `the previous summary left ${afterTokens} tokens above the new ${triggerTokens}-token trigger`,
  )
  assert.equal(requests.length, 1, 'updating the previous summary needs only one request')
  const requestText = JSON.stringify(requests[0])
  assert.ok(requestText.includes('Preserve the earlier verified decision.'))
  assert.ok(requestText.includes('Continue the remaining fixture work.'))
  assert.ok(compaction.usage.totalTokens > 0, 'the replacement summary usage must be retained')
  assert.equal(compaction.firstKeptEntryId, preparation.firstKeptEntryId)
  const projected = manager.buildSessionContext().messages
  assert.deepEqual(projected.slice(-retainedMessages.length), retainedMessages)
  assertPairedToolResults(projected)
  appendToolTurn(manager, 100, 200)
  assert.equal(shouldCompact(projectedTokens(manager), model.contextWindow, settings), false)
})
