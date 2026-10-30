import assert from 'node:assert/strict'
import test from 'node:test'
import { ASK_USER_TOOL_NAME, createAskUserTool } from '../tools/app/ask-user.mjs'
import {
  QUESTION_LIMITS,
  QuestionInputError,
  SessionQuestionService,
  normalizeAnswers,
  normalizeQuestions,
  projectQuestionSummary,
} from '../services/session-question-service.mjs'
import { projectSessionOrganization } from '../services/chat-session-organization.mjs'
import { projectSessionCard } from '../services/session-card-summary.mjs'
import { channelQuestionText } from '../services/channels/channel-service.mjs'
import { permissionRequirement } from '../services/session-permission-service.mjs'
import { filterToolsForExecutionMode } from '../security/execution-mode.mjs'
import { hotToolNames } from '../tools/tool-activation.mjs'
import { ToolActivation } from '../runtime/tool-activation.mjs'
import { resolveRuntimeCapabilities } from '../runtime-capabilities.mjs'

const QUESTIONS = [
  {
    header: 'Auth',
    question: 'Which auth method should the API use?',
    options: [
      { label: 'OAuth', description: 'Recommended for third parties' },
      { label: 'API key' },
    ],
  },
  {
    question: 'Which environments should get it?',
    options: ['staging', 'production', 'preview'],
    multiSelect: true,
  },
]

async function waitForPending(service, sessionId) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const pending = service.getPending(sessionId)
    if (pending.length) return pending
    await new Promise((resolve) => setTimeout(resolve, 5))
  }
  assert.fail('question never became pending')
}

test('normalizeQuestions trims, limits and validates agent input', () => {
  const [first, second] = normalizeQuestions(QUESTIONS)
  assert.deepEqual(first, {
    header: 'Auth',
    question: 'Which auth method should the API use?',
    options: [
      { label: 'OAuth', description: 'Recommended for third parties' },
      { label: 'API key' },
    ],
    multiSelect: false,
  })
  // 没有标题时由问题正文截出标题；字符串选项也接受。
  assert.equal(second.header, 'Which environments shoul')
  assert.equal(Array.from(second.header).length, QUESTION_LIMITS.header)
  assert.deepEqual(
    second.options.map((option) => option.label),
    ['staging', 'production', 'preview'],
  )
  assert.equal(second.multiSelect, true)

  const long = normalizeQuestions([
    { question: `${'问'.repeat(500)}\u0007`, options: ['甲'.repeat(80), 'b'] },
  ])[0]
  assert.equal(Array.from(long.question).length, QUESTION_LIMITS.question)
  assert.equal(Array.from(long.options[0].label).length, QUESTION_LIMITS.label)
  // 孤立代理项被替换，输出严格合法。
  const lone = normalizeQuestions([{ question: 'x\uD800y', options: ['a', 'b'] }])[0]
  assert.doesNotThrow(() => JSON.parse(JSON.stringify(lone)))
  assert.equal(lone.question.includes('\uD800'), false)

  for (const [input, code] of [
    [[], 'questions_required'],
    ['nope', 'questions_required'],
    [Array.from({ length: 5 }, () => QUESTIONS[0]), 'too_many_questions'],
    [[{ question: '', options: ['a', 'b'] }], 'question_text_required'],
    [[{ question: 'q', options: ['a'] }], 'option_count'],
    [[{ question: 'q', options: ['a', 'b', 'c', 'd', 'e', 'f', 'g'] }], 'option_count'],
    [[{ question: 'q', options: ['a', 'a'] }], 'duplicate_option'],
    [[{ question: 'q', options: ['a', { label: ' ' }] }], 'option_label_required'],
  ]) {
    assert.throws(
      () => normalizeQuestions(input),
      (error) => error instanceof QuestionInputError && error.code === code,
    )
  }
})

test('normalizeAnswers enforces selection rules and free text', () => {
  const questions = normalizeQuestions(QUESTIONS)
  assert.deepEqual(normalizeAnswers(questions, { skip: true }), { skip: true })
  assert.deepEqual(
    normalizeAnswers(questions, {
      answers: [{ selected: ['OAuth'] }, { selected: [], other: '  only staging for now ' }],
    }),
    {
      skip: false,
      answers: [{ selected: ['OAuth'] }, { selected: [], other: 'only staging for now' }],
    },
  )
  const cases = [
    [{ answers: [{ selected: ['OAuth'] }] }, 'answers_mismatch'],
    [{}, 'answers_mismatch'],
    [{ answers: [{ selected: ['OAuth', 'API key'] }, { selected: ['staging'] }] }, 'single_select'],
    [{ answers: [{ selected: ['Nope'] }, { selected: ['staging'] }] }, 'invalid_selection'],
    [{ answers: [{ selected: [1] }, { selected: ['staging'] }] }, 'invalid_selection'],
    [
      { answers: [{ selected: ['OAuth'] }, { selected: ['staging', 'staging'] }] },
      'invalid_selection',
    ],
    [{ answers: [{ selected: [] }, { selected: ['staging'] }] }, 'answer_required'],
    [
      { answers: [{ selected: ['OAuth'], other: 'x'.repeat(2001) }, { selected: ['staging'] }] },
      'other_too_long',
    ],
  ]
  for (const [input, code] of cases) {
    assert.throws(
      () => normalizeAnswers(questions, input),
      (error) => error instanceof QuestionInputError && error.code === code,
      code,
    )
  }
})

test('question service resolves once, emits events and keeps a conflict record', async () => {
  const observed = []
  const streamed = []
  const service = new SessionQuestionService({
    onEvent: (sessionId, event, data) => observed.push([sessionId, event, data.outcome || '']),
  })
  service.attachEmitter('s1', (event, data) => streamed.push([event, data.id]))
  const questions = normalizeQuestions(QUESTIONS)
  const pending = service.request({ sessionId: 's1', toolCallId: 'call-1', questions })
  const [request] = await waitForPending(service, 's1')
  assert.match(request.id, /^q_/)
  assert.equal(request.questionId, request.id)
  assert.equal(request.toolCallId, 'call-1')
  assert.deepEqual(service.getPending('other'), [])
  // 输入非法：问题仍在等待。
  assert.throws(() => service.answer('s1', request.id, { answers: [] }), QuestionInputError)
  assert.equal(service.getPending('s1').length, 1)
  // 别的会话不能回答它。
  assert.equal(service.answer('s2', request.id, { skip: true }).found, false)
  const answered = service.answer('s1', request.id, {
    answers: [{ selected: ['API key'] }, { selected: ['staging', 'preview'] }],
  })
  assert.equal(answered.found, true)
  assert.equal(answered.alreadyResolved, false)
  assert.equal(answered.outcome, 'answered')
  assert.deepEqual(await pending, {
    outcome: 'answered',
    answers: [{ selected: ['API key'] }, { selected: ['staging', 'preview'] }],
  })
  const repeat = service.answer('s1', request.id, { skip: true })
  assert.equal(repeat.alreadyResolved, true)
  assert.equal(repeat.outcome, 'answered')
  assert.deepEqual(service.getPending('s1'), [])
  assert.deepEqual(
    streamed.map(([event]) => event),
    ['question_request', 'question_resolved'],
  )
  assert.deepEqual(observed, [
    ['s1', 'question_request', ''],
    ['s1', 'question_resolved', 'answered'],
  ])
  assert.equal(service.answer('s1', 'q_unknown-question', { skip: true }).found, false)
  service.dispose()
})

test('stopping the run or the session cancels pending questions; skip is reported', async () => {
  const service = new SessionQuestionService()
  const questions = normalizeQuestions(QUESTIONS.slice(0, 1))
  const controller = new AbortController()
  const viaSignal = service.request({ sessionId: 's', questions, signal: controller.signal })
  await waitForPending(service, 's')
  controller.abort()
  assert.equal((await viaSignal).outcome, 'cancelled')
  assert.deepEqual(service.getPending('s'), [])

  const aborted = new AbortController()
  aborted.abort()
  assert.equal(
    (await service.request({ sessionId: 's', questions, signal: aborted.signal })).outcome,
    'cancelled',
  )

  const viaStop = service.request({ sessionId: 's', questions })
  await waitForPending(service, 's')
  assert.equal(service.cancelSession('s', '运行已停止。'), 1)
  assert.deepEqual(await viaStop, { outcome: 'cancelled', reason: '运行已停止。' })

  const skipped = service.request({ sessionId: 's', questions })
  const [pending] = await waitForPending(service, 's')
  service.answer('s', pending.id, { skip: true })
  assert.deepEqual(await skipped, { outcome: 'skipped' })
  service.dispose()
})

test('ask_user tool blocks until answered and tells the agent what happened', async () => {
  const service = new SessionQuestionService()
  let available = true
  const tool = createAskUserTool({
    isAvailable: () => available,
    requestAnswers: (input) => service.request({ sessionId: 's', ...input }),
  })
  assert.equal(tool.name, ASK_USER_TOOL_NAME)
  const running = tool.execute('call-9', { questions: QUESTIONS })
  const [pending] = await waitForPending(service, 's')
  service.answer('s', pending.id, {
    answers: [{ selected: ['OAuth'], other: 'and rotate keys' }, { selected: ['production'] }],
  })
  const result = await running
  assert.equal(result.isError, undefined)
  const payload = JSON.parse(result.content[0].text)
  assert.deepEqual(payload, {
    outcome: 'answered',
    answers: [
      {
        question: 'Which auth method should the API use?',
        selected: ['OAuth'],
        other: 'and rotate keys',
      },
      { question: 'Which environments should get it?', selected: ['production'] },
    ],
  })
  assert.equal(result.details.outcome, 'answered')

  const skipping = tool.execute('call-10', { questions: QUESTIONS.slice(0, 1) })
  const [second] = await waitForPending(service, 's')
  service.answer('s', second.id, { skip: true })
  assert.match((await skipping).content[0].text, /skipped/)

  const invalid = await tool.execute('call-11', { questions: [{ question: 'q', options: ['a'] }] })
  assert.equal(invalid.isError, true)
  assert.match(invalid.content[0].text, /2–6 options/)

  // 没人在场的运行：工具直接告诉代理自己决定，不登记问题。
  available = false
  const unattended = await tool.execute('call-12', { questions: QUESTIONS })
  assert.equal(unattended.isError, true)
  assert.deepEqual(service.getPending('s'), [])
  service.dispose()
})

test('ask_user is never gated by approvals and stays visible in read-only mode', () => {
  for (const executionMode of ['approval-required', 'workspace-write', 'full-access']) {
    assert.equal(
      permissionRequirement({
        mode: 'ask',
        executionMode,
        cwd: process.cwd(),
        toolName: ASK_USER_TOOL_NAME,
        args: { questions: QUESTIONS },
      }),
      null,
    )
  }
  assert.deepEqual(filterToolsForExecutionMode([ASK_USER_TOOL_NAME], 'approval-required'), [
    ASK_USER_TOOL_NAME,
  ])
  assert.deepEqual(hotToolNames(['read', ASK_USER_TOOL_NAME, 'web_search']), [
    'read',
    ASK_USER_TOOL_NAME,
  ])
})

test('unattended runs do not offer ask_user', () => {
  const activation = new ToolActivation({
    getExecutionMode: () => 'workspace-write',
    getToolRisk: () => 'low',
    getSessionMeta: () => ({}),
    saveSessionMeta: async () => {},
    getGoal: () => null,
    promoteTools: async () => ({ routedToolNames: [] }),
  })
  const active = []
  const session = {
    sessionId: 's',
    getAllTools: () => [{ name: 'read' }, { name: ASK_USER_TOOL_NAME }],
    setActiveToolsByName: (names) => active.splice(0, active.length, ...names),
  }
  const value = { session, baseToolNames: ['read'] }
  activation.syncGoalTools(value, null)
  assert.ok(active.includes(ASK_USER_TOOL_NAME))
  assert.ok(activation.optionalToolNames(value).includes(ASK_USER_TOOL_NAME))
  value.unattendedRun = true
  activation.syncGoalTools(value, null)
  assert.equal(active.includes(ASK_USER_TOOL_NAME), false)
  assert.equal(activation.optionalToolNames(value).includes(ASK_USER_TOOL_NAME), false)
})

test('pending questions put the session in needs-you behind approvals', () => {
  assert.equal(projectSessionOrganization({}, 0, 1).attentionReason, 'question')
  assert.equal(projectSessionOrganization({}, 0, 1).needsAttention, true)
  assert.equal(projectSessionOrganization({}, 1, 1).attentionReason, 'approval')
  assert.equal(
    projectSessionOrganization({ organization: { failed: true } }, 0, 1).attentionReason,
    'question',
  )
  // 旧调用方（两个参数）不受影响。
  assert.equal(projectSessionOrganization({}, 0).attentionReason, null)
})

test('question summary feeds the session list and the board card', () => {
  assert.equal(projectQuestionSummary([]), null)
  const questions = normalizeQuestions(QUESTIONS)
  const summary = projectQuestionSummary([
    { id: 'q_late-question', questions: questions.slice(1), createdAt: '2026-10-30T10:05:00.000Z' },
    { id: 'q_early-question', questions, createdAt: '2026-10-30T10:00:00.000Z' },
  ])
  assert.equal(summary.id, 'q_early-question')
  assert.equal(summary.count, 2)
  assert.equal(summary.prompt, 'Which auth method should the API use?')
  assert.deepEqual(summary.options, ['OAuth', 'API key'])
  assert.equal(summary.allowFreeText, true)
  assert.equal(summary.multiSelect, false)

  const session = {
    id: 's',
    streaming: true,
    needsAttention: true,
    attentionReason: 'question',
    modified: '2026-10-30T10:00:00.000Z',
  }
  const card = projectSessionCard({
    session,
    question: summary,
    capabilities: { askUser: true },
    now: Date.parse('2026-10-30T10:10:00.000Z'),
  })
  assert.equal(card.boardStatus, 'attention')
  assert.equal(card.question.id, 'q_early-question')
  assert.doesNotThrow(() => JSON.parse(JSON.stringify(card)))
  // 没有 askUser 能力的投影不把问题算作需要你（旧行为）。
  const legacy = projectSessionCard({ session, question: summary, now: Date.now() })
  assert.equal(legacy.boardStatus, 'running')
})

test('channels only relay the question text and point to the app', () => {
  const text = channelQuestionText({ questions: normalizeQuestions(QUESTIONS) })
  assert.match(text, /1\. Which auth method/)
  assert.match(text, /A\. OAuth/)
  assert.match(text, /2\. Which environments should get it\?（可多选）/)
  assert.match(text, /请在 Pisper 应用里点选回答/)
  assert.equal(channelQuestionText({}).includes('请在 Pisper 应用里'), true)
})

test('askUser capability is declared on every profile', async () => {
  for (const profile of ['desktop', 'mobile-embedded', 'mobile-store']) {
    const capabilities = await resolveRuntimeCapabilities({
      environment: { PISPER_RUNTIME_PROFILE: profile },
      moduleSupport: { childProcess: true, workerThreads: true, sqlite: true, wasm: true },
    })
    assert.equal(capabilities.profile, profile)
    assert.equal(capabilities.features.askUser, true, profile)
  }
})
