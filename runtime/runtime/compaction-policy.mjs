// 压缩策略：包装 Pi 引擎的压缩设置与触发逻辑，加入 Pisper 侧的阈值/预留 token 策略，
// 并确保压缩发生在回合边界（工具结果落地之后），避免上下文被中途改写。
import { isAbsolute, resolve } from 'node:path'
import { SUPERSEDED_READ } from '../prompts/sessions.mjs'
import { compact } from './pi-coding-agent.mjs'

export const DEFAULT_COMPACTION_THRESHOLD_PERCENT = 80
// 下限 10：允许激进压缩策略（小窗口模型/长工具输出场景）；上限 95 保留，
// 再高会挤掉摘要响应所需的预留 token。
export const MIN_COMPACTION_THRESHOLD_PERCENT = 10
export const MAX_COMPACTION_THRESHOLD_PERCENT = 95
// 摘要响应的预留 token：压缩时不能把上下文窗口全部占满，否则摘要无法生成。
export const COMPACTION_SUMMARY_RESERVE_TOKENS = 16_384
// 压缩触发线的绝对上限：1M 窗口的模型按 80% 要到约 838K 才压缩，单次请求又慢又贵，
// 模型效果也会下降。触发线取“窗口 × 阈值”与这个上限中较小的一个；0 表示不设上限。
export const DEFAULT_COMPACTION_MAX_CONTEXT_TOKENS = 256_000
export const COMPACTION_MAX_CONTEXT_TOKEN_OPTIONS = Object.freeze([128_000, 256_000, 512_000, 0])
const MIN_COMPACTION_MAX_CONTEXT_TOKENS = 32_000
const MAX_COMPACTION_MAX_CONTEXT_TOKENS = 10_000_000

const PISPER_TURN_COMPACTION_PATCH = Symbol('pisper.turn-compaction-patch')

function tokenCount(value, fallback = 0) {
  const number = Math.floor(Number(value) || 0)
  return number > 0 ? number : fallback
}

// 把阈值限制到 [MIN, MAX] 区间，非法输入回退到默认值。
export function normalizeCompactionThresholdPercent(
  value,
  fallback = DEFAULT_COMPACTION_THRESHOLD_PERCENT,
) {
  const number = Number(value)
  if (!Number.isFinite(number)) return fallback
  return Math.min(
    MAX_COMPACTION_THRESHOLD_PERCENT,
    Math.max(MIN_COMPACTION_THRESHOLD_PERCENT, Math.round(number)),
  )
}

// 上限只接受 0（不限）或合理区间内的整数，其余回退到默认值。
export function normalizeCompactionMaxContextTokens(
  value,
  fallback = DEFAULT_COMPACTION_MAX_CONTEXT_TOKENS,
) {
  if (value === 0 || value === '0') return 0
  const number = Number(value)
  if (
    !Number.isSafeInteger(number) ||
    number < MIN_COMPACTION_MAX_CONTEXT_TOKENS ||
    number > MAX_COMPACTION_MAX_CONTEXT_TOKENS
  )
    return fallback
  return number
}

// 计算有效压缩设置：由阈值百分比反推预留 token（窗口余量），
// 未显式配置时兜底使用默认预留/保留最近 token 数。
export function effectiveCompactionSettings(
  settings = {},
  contextWindow = 0,
  thresholdPercent = DEFAULT_COMPACTION_THRESHOLD_PERCENT,
  maxContextTokens = DEFAULT_COMPACTION_MAX_CONTEXT_TOKENS,
) {
  const windowTokens = tokenCount(contextWindow)
  const normalizedThreshold = normalizeCompactionThresholdPercent(thresholdPercent)
  const cap = normalizeCompactionMaxContextTokens(maxContextTokens)
  const percentTrigger = Math.floor((windowTokens * normalizedThreshold) / 100)
  const triggerTokens = cap ? Math.min(percentTrigger, cap) : percentTrigger
  const thresholdReserve = windowTokens ? Math.max(1, windowTokens - triggerTokens) : 0
  const keepRecentTokens = tokenCount(settings.keepRecentTokens, 20_000)
  // 保留尾部不能反过来超过压缩触发线；至少留下一半给摘要、系统上下文和后续工作。
  const retainedLimit = windowTokens
    ? Math.max(1, Math.floor((windowTokens - thresholdReserve) / 2))
    : keepRecentTokens
  return {
    ...settings,
    enabled: settings.enabled !== false,
    reserveTokens:
      thresholdReserve || tokenCount(settings.reserveTokens, COMPACTION_SUMMARY_RESERVE_TOKENS),
    keepRecentTokens: Math.min(keepRecentTokens, retainedLimit),
  }
}

// 用 Proxy 覆盖 SettingsManager 的 getCompactionSettings：
// 让会话的压缩设置动态反映当前模型上下文窗口与全局阈值，无需每次手动传入。
export function createCompactionSettingsManager(
  settingsManager,
  getContextWindow = () => 0,
  getThresholdPercent = () => DEFAULT_COMPACTION_THRESHOLD_PERCENT,
  getMaxContextTokens = () => DEFAULT_COMPACTION_MAX_CONTEXT_TOKENS,
) {
  if (!settingsManager) return settingsManager
  return new Proxy(settingsManager, {
    get(target, property) {
      if (property === 'getCompactionSettings') {
        return () =>
          effectiveCompactionSettings(
            target.getCompactionSettings(),
            getContextWindow(),
            getThresholdPercent(),
            getMaxContextTokens(),
          )
      }
      const value = Reflect.get(target, property, target)
      return typeof value === 'function' ? value.bind(target) : value
    },
  })
}

// 回合边界压缩补丁：在 prepareNextTurnWithContext（工具结果后的下一轮准备）时
// 先检查压缩；若发生了压缩（叶子节点变化），用压缩后的上下文替换本轮快照，
// 避免下一轮基于压缩前的旧上下文继续。
export function installTurnBoundaryCompaction(session) {
  if (
    !session?.agent ||
    session[PISPER_TURN_COMPACTION_PATCH] ||
    typeof session._checkCompaction !== 'function' ||
    typeof session.sessionManager?.buildSessionContext !== 'function'
  )
    return session

  const previousPrepareNextTurnWithContext = session.agent.prepareNextTurnWithContext
  session.agent.prepareNextTurnWithContext = async (turn, signal) => {
    const previousSnapshot = await previousPrepareNextTurnWithContext?.(turn, signal)
    if (!turn.toolResults?.length) return previousSnapshot

    const previousContext = previousSnapshot?.context ?? turn.context
    const leafBefore = session.sessionManager.getLeafId?.()
    await session._checkCompaction(turn.message)
    const leafAfter = session.sessionManager.getLeafId?.()
    if (leafBefore === leafAfter) return previousSnapshot

    return {
      ...previousSnapshot,
      context: {
        ...previousContext,
        messages: session.sessionManager.buildSessionContext().messages,
      },
    }
  }
  session[PISPER_TURN_COMPACTION_PATCH] = true
  return session
}

// 压缩扩展：让 Pi 引擎在 session_before_compact 钩子中使用 Pisper 的压缩实现，
// 并收紧预留 token——更早的压缩不应同时放大摘要响应的可用空间。
// 设置了辅助模型时先用它写摘要；取不到认证或生成失败（例如上下文装不下）再回到会话模型。
// 预留量与触发线始终按会话模型计算：压缩是为会话模型腾出空间。
export function pisperCompactionExtension(
  pi,
  { compactSession = compact, resolveAuxiliaryModel = async () => null } = {},
) {
  // 压缩保留段的读取去重：只在发给模型的上下文里投影，不改会话文件。
  pi.on('context', async (event, context) => {
    let cwd = ''
    try {
      cwd = String(context?.cwd || '')
    } catch {
      // 扩展上下文已失效时按相对路径原样比较，宁可少去重。
    }
    const messages = dedupeKeptReads(event.messages, { cwd })
    return messages === event.messages ? undefined : { messages }
  })
  pi.on('session_before_compact', async (event, context) => {
    const model = context.model
    if (!model) return undefined
    const reserveTokens = tokenCount(
      event.preparation.settings?.reserveTokens,
      COMPACTION_SUMMARY_RESERVE_TOKENS,
    )
    const contextWindow = tokenCount(model.contextWindow)
    // Pi 的分段压缩最多合并 0.8 + 0.5 倍预留量的两份摘要。
    // 按触发线的四分之一限制预留量，避免摘要与保留尾部再次填满触发线。
    const summaryReserveLimit = contextWindow
      ? Math.max(2, Math.floor(Math.max(1, contextWindow - reserveTokens) / 4))
      : COMPACTION_SUMMARY_RESERVE_TOKENS
    // 只有当前长回合的前缀时，Pi 会直接复制旧摘要再拼接新摘要。
    // 改为用该前缀更新旧摘要，保证切换模型后旧摘要也遵守当前输出预算。
    const summarizePreviousWithPrefix =
      event.preparation.isSplitTurn &&
      event.preparation.previousSummary &&
      event.preparation.messagesToSummarize.length === 0
    const preparation = {
      ...event.preparation,
      ...(summarizePreviousWithPrefix
        ? {
            isSplitTurn: false,
            messagesToSummarize: event.preparation.turnPrefixMessages,
            turnPrefixMessages: [],
          }
        : {}),
      settings: {
        ...event.preparation.settings,
        reserveTokens: Math.min(
          reserveTokens,
          COMPACTION_SUMMARY_RESERVE_TOKENS,
          summaryReserveLimit,
        ),
      },
    }
    // Pi 1.4 的摘要生成改用模型 maxTokens，不再读取 reserveTokens。
    // 只限制本次摘要的模型副本，保留旧版 80% 输出预算，不修改会话模型。
    const summarize = async (summaryModelSource, auth) =>
      compactSession(
        preparation,
        {
          ...summaryModelSource,
          maxTokens: Math.min(
            tokenCount(summaryModelSource.maxTokens, Number.MAX_SAFE_INTEGER),
            Math.max(1, Math.floor(preparation.settings.reserveTokens * 0.8)),
          ),
        },
        auth.apiKey,
        auth.headers,
        event.customInstructions,
        event.signal,
        'off',
        undefined,
        auth.env,
      )
    const auxiliary = await Promise.resolve()
      .then(resolveAuxiliaryModel)
      .catch(() => null)
    if (auxiliary && !sameModel(auxiliary, model)) {
      const auxiliaryAuth = await context.modelRegistry
        .getApiKeyAndHeaders(auxiliary)
        .catch(() => null)
      if (auxiliaryAuth?.ok) {
        try {
          return { compaction: await summarize(auxiliary, auxiliaryAuth) }
        } catch (error) {
          if (event.signal?.aborted) throw error
        }
      }
    }
    const auth = await context.modelRegistry.getApiKeyAndHeaders(model)
    if (!auth?.ok) return undefined
    return { compaction: await summarize(model, auth) }
  })
}

function sameModel(left, right) {
  return left?.provider === right?.provider && left?.id === right?.id
}

// ---- 压缩保留段的读取去重 ----
// 压缩后留下的最近一段对话里，同一文件常被读了好几次，早先那几份内容已经过时，
// 却和最新的一份一起占着上下文。这里把被后面读取完整覆盖的旧结果换成一行占位。
// 只动“压缩时保留下来”的那一段：摘要之后、时间戳不晚于压缩时刻的消息。
// 这一段在两次压缩之间固定不变，所以每次请求投影出的前缀完全相同，不打断提示词缓存；
// 压缩之后新产生的消息一律不动，否则每读一次文件，前面的历史就会变，整段缓存失效。

const READ_TOOL = 'read'
const CONTINUATION_MARKER = /Use offset=\d+ to continue/

function finiteNumber(value) {
  const number = Number(value)
  return Number.isFinite(number) ? number : undefined
}

// 读取范围：offset 从 1 开始，缺省为整文件开头；limit 缺省为不限。
function readRange(args) {
  const offset = finiteNumber(args?.offset)
  const limit = finiteNumber(args?.limit)
  return {
    offset: offset && offset > 1 ? Math.floor(offset) : 1,
    limit: limit && limit > 0 ? Math.floor(limit) : 0,
  }
}

function readPathKey(path, cwd) {
  const raw = typeof path === 'string' ? path.trim().replace(/^@/, '') : ''
  if (!raw) return ''
  if (isAbsolute(raw)) return resolve(raw)
  return cwd ? resolve(cwd, raw) : raw
}

function textOnly(content) {
  return (
    Array.isArray(content) &&
    content.length > 0 &&
    content.every((part) => part?.type === 'text' && typeof part.text === 'string')
  )
}

// 结果是否就是完整文件：没有指定范围，也没被截断（截断时 Pi 会提示用 offset 继续）。
function isCompleteRead(read) {
  return read.range.offset === 1 && !read.range.limit && !read.truncated
}

function covers(later, earlier) {
  if (isCompleteRead(later)) return true
  return later.range.offset === earlier.range.offset && later.range.limit === earlier.range.limit
}

/**
 * 压缩保留段里，把被同一文件后续读取覆盖的旧读取结果换成占位。纯函数：没有可替换的内容时
 * 原样返回同一个数组，便于调用方判断是否改动。
 * @param {any[]} messages 发给模型前的消息（含 compactionSummary）
 * @param {{ cwd?: string }} [options]
 */
export function dedupeKeptReads(messages, { cwd = '' } = {}) {
  if (!Array.isArray(messages)) return messages
  let summaryIndex = -1
  for (let index = messages.length - 1; index >= 0; index--) {
    if (messages[index]?.role === 'compactionSummary') {
      summaryIndex = index
      break
    }
  }
  if (summaryIndex < 0) return messages
  const compactedAt = finiteNumber(messages[summaryIndex].timestamp)
  if (compactedAt === undefined) return messages
  let end = summaryIndex + 1
  while (end < messages.length) {
    const timestamp = finiteNumber(messages[end]?.timestamp)
    if (timestamp === undefined || timestamp > compactedAt) break
    end += 1
  }

  const calls = new Map()
  const reads = []
  for (let index = summaryIndex + 1; index < end; index++) {
    const message = messages[index]
    if (message?.role === 'assistant' && Array.isArray(message.content)) {
      for (const part of message.content) {
        if (part?.type === 'toolCall' && part.name === READ_TOOL && part.id)
          calls.set(part.id, part.arguments)
      }
      continue
    }
    if (message?.role !== 'toolResult' || message.toolName !== READ_TOOL || message.isError)
      continue
    if (!calls.has(message.toolCallId) || !textOnly(message.content)) continue
    const args = calls.get(message.toolCallId)
    const key = readPathKey(args?.path, cwd)
    if (!key) continue
    reads.push({
      index,
      key,
      path: String(args.path).trim(),
      range: readRange(args),
      truncated:
        message.details?.truncation?.truncated === true ||
        message.content.some((part) => CONTINUATION_MARKER.test(part.text)),
    })
  }

  const superseded = new Map()
  for (let position = 0; position < reads.length; position++) {
    const earlier = reads[position]
    for (let next = position + 1; next < reads.length; next++) {
      const later = reads[next]
      if (later.key === earlier.key && covers(later, earlier)) {
        superseded.set(earlier.index, earlier.path)
        break
      }
    }
  }
  if (!superseded.size) return messages
  return messages.map((message, index) =>
    superseded.has(index)
      ? {
          ...message,
          content: [{ type: 'text', text: SUPERSEDED_READ(superseded.get(index)) }],
        }
      : message,
  )
}
