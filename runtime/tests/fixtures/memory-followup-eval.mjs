import assert from 'node:assert/strict'
import { resolve } from 'node:path'
import { answers, questions, unanswered } from './memory-pipeline-cases.mjs'

const unfinished = questions.find(({ key }) => key === 'unfinished-original').text
const config = '我们刚才在讨论配置来源校验。'
const mirror = '我们刚才在讨论玩家镜像快照。'
const cases = [
  ['unfinished', config, unfinished, 'unfinished'],
  ['time', mirror, '那是几点跑的？', 'schedule'],
  ['archive-time', mirror, '那是几点跑的？', 'otherSchedule', 'archive'],
  ['wrong-topic', '我们刚才在讨论快照导出。', unfinished],
  ['mixed-topic', '我们刚才在讨论快照压缩。', '那是几点跑的？'],
  ['no-context', '', unfinished],
  ['new-topic', config, '另外，晚饭想吃什么？'],
  ['different-session', config, unfinished, null, 'drill', 'other-session'],
  ['different-workspace', config, unfinished, null, 'drill', 'new-session', 'archive'],
  ['private-chat', '我们刚才在讨论棋盘边长。', unfinished, null, 'chat', 'board'],
  [
    'same-private-chat',
    '我们刚才在讨论棋盘边长。',
    unfinished,
    null,
    'chat',
    'board',
    'chat',
    'board',
  ],
  ...unanswered.map(([key, text]) => [key, mirror, text]),
  ['missing-time', '我们刚才在讨论快照清理。', '那是几点跑的？'],
]

// 在重试规则确定后另列日常对话，只用于观察泛化，不参与规则调整。
const heldOutCases = [
  ['review-implementation', '配置来源校验这一块，你能帮我过一遍吗？', unfinished, 'unfinished'],
  ['night-shift', '今晚我要值班，演练服的玩家镜像快照我得盯一下。', '那是几点跑的？', 'schedule'],
  [
    'archive-handoff',
    '说完索引，接下来我想问归档站的镜像快照。',
    '它是几点运行的？',
    'otherSchedule',
    'archive',
  ],
  ['ambiguous-job', '玩家镜像快照和快照清理是两个不同的任务吧？', '那是几点跑的？'],
]

// 只解析短、显式的单主题指代。完整匹配问句，避免“实现要多少钱”等额外条件被吞掉。
function retryPlan(current, previous) {
  if (!previous || previous.workspace !== current.workspace || previous.session !== current.session)
    return null
  const topic =
    /^(?:(?:我们)?(?:刚才)?在讨论|说说|聊聊)([\p{Script=Han}A-Za-z0-9_]{2,24})[。？?]?$/u.exec(
      previous.text,
    )?.[1]
  if (!topic) return null
  if (/^(?:还有|那)?哪些(?:还)?(?:没|未)实现[？?]?$/u.test(current.text))
    return { topic, evidence: /(?:尚未|还没|未|没)(?:有)?实现/u }
  if (/^(?:那|它)(?:是)?几点(?:跑|运行)(?:的)?[？?]?$/u.test(current.text))
    return { topic, evidence: /(?:[01]?\d|2[0-3])[:：][0-5]\d/u }
  return null
}

async function measureCases(memory, renderedFacts, samples) {
  const result = {
    positives: 0,
    negatives: 0,
    baselineHits: [],
    candidateHits: [],
    baselineFalse: [],
    candidateFalse: [],
    newWrongRows: [],
    leaks: [],
    retries: [],
  }
  for (const [
    key,
    prior,
    text,
    target,
    workspace = 'drill',
    previousSession = 'new-session',
    previousWorkspace = workspace,
    session = 'new-session',
  ] of samples) {
    const recall = async (query) =>
      renderedFacts(
        await memory.relevantContext(query, resolve('memory-eval', workspace), undefined, {
          sessionId: session,
        }),
      )
    const baseline = await recall(text)
    let retried = []
    let candidate = baseline
    const plan = retryPlan(
      { text, workspace, session },
      { text: prior, workspace: previousWorkspace, session: previousSession },
    )
    if (!baseline.length && plan) {
      result.retries.push(key)
      retried = await recall(plan.topic + ' ' + text)
      candidate = retried.filter(
        (item) =>
          item.text.includes(plan.topic) &&
          plan.evidence.test(item.text.replaceAll(plan.topic, '')),
      )
    }
    const answer = answers.find((item) => item.key === target)
    const correct = ({ row, text: body }) =>
      answer &&
      row.sessionId === answer.source &&
      answer.patterns.every((pattern) => pattern.test(body))
    if (answer) {
      result.positives++
      if (baseline.some(correct)) result.baselineHits.push(key)
      if (candidate.some(correct)) result.candidateHits.push(key)
    } else {
      result.negatives++
      if (baseline.length) result.baselineFalse.push(key)
      if (candidate.length) result.candidateFalse.push(key)
    }
    if (
      candidate.some(
        (item) => !correct(item) && !baseline.some(({ row }) => row.id === item.row.id),
      )
    )
      result.newWrongRows.push(key)
    if (
      [...baseline, ...retried].some(
        ({ row }) =>
          !answers.some(
            ({ source, scope }) =>
              source === row.sessionId &&
              (scope === 'global' ||
                (scope === workspace && (scope !== 'chat' || row.sessionId === session))),
          ),
      )
    )
      result.leaks.push(key)
  }
  return result
}

export async function measureFollowups(memory, renderedFacts) {
  const controlled = await measureCases(memory, renderedFacts, cases)
  for (const key of ['different-session', 'different-workspace', 'private-chat'])
    assert.ok(!controlled.retries.includes(key), key)
  assert.ok(controlled.retries.includes('same-private-chat'), 'same-private-chat')
  assert.ok(!controlled.candidateFalse.includes('same-private-chat'), 'same-private-chat')
  const heldOut = await measureCases(memory, renderedFacts, heldOutCases)
  return { ...controlled, heldOut }
}
