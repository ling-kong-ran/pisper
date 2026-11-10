import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler, requiredRuntimeFeature } from '../http/api-handler.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  SessionQuestionService,
  normalizeQuestions,
} from '../services/session-question-service.mjs'

async function post(handler, path, payload) {
  const request = {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    async *[Symbol.asyncIterator]() {
      yield Buffer.from(typeof payload === 'string' ? payload : JSON.stringify(payload))
    },
  }
  const response = {
    status: 0,
    body: '',
    writeHead(status) {
      this.status = status
    },
    setHeader() {},
    end(body) {
      this.body = body
    },
  }
  const handled = await handler(request, response, new URL(`http://localhost${path}`))
  assert.equal(handled, true)
  return { status: response.status, body: JSON.parse(response.body) }
}

function setup({ askUser = true } = {}) {
  const questions = new SessionQuestionService()
  const runtime = {
    capabilities: { features: { askUser } },
    answerSessionQuestion: (sessionId, questionId, input) =>
      questions.answer(sessionId, questionId, input),
  }
  return { questions, handler: createApiHandler(runtime) }
}

async function pendingQuestion(questions, sessionId, input) {
  const result = questions.request({ sessionId, questions: normalizeQuestions(input) })
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const [pending] = questions.getPending(sessionId)
    if (pending) return { pending, result }
    await new Promise((resolve) => setTimeout(resolve, 5))
  }
  assert.fail('question never became pending')
}

test('question routes are gated on the askUser capability', async () => {
  assert.equal(requiredRuntimeFeature('/api/sessions/s1/questions/q_abcdefgh', 'POST'), 'askUser')
  assert.equal(requiredRuntimeFeature('/api/sessions/s1/approvals/a1', 'POST'), '')
  const { handler } = setup({ askUser: false })
  const response = await post(handler, '/api/sessions/s1/questions/q_abcdefgh', { skip: true })
  assert.equal(response.status, 409)
  assert.equal(response.body.capability, 'askUser')
})

test('answering a single-select question returns 200, then 409 on repeat', async () => {
  const { questions, handler } = setup()
  const { pending, result } = await pendingQuestion(questions, 's1', [
    { question: 'Ship today?', options: ['Yes', 'No'] },
  ])
  const path = `/api/sessions/s1/questions/${pending.id}`
  const ok = await post(handler, path, { answers: [{ selected: ['Yes'] }] })
  assert.equal(ok.status, 200)
  assert.equal(ok.body.outcome, 'answered')
  assert.deepEqual(await result, { outcome: 'answered', answers: [{ selected: ['Yes'] }] })
  const again = await post(handler, path, { answers: [{ selected: ['No'] }] })
  assert.equal(again.status, 409)
  assert.equal(again.body.code, 'question_already_resolved')
  assert.equal(again.body.outcome, 'answered')
  questions.dispose()
})

test('invalid answers are rejected with structured 400 and keep the question pending', async () => {
  const { questions, handler } = setup()
  const { pending, result } = await pendingQuestion(questions, 's2', [
    { question: 'Which?', options: ['A', 'B'] },
    { question: 'Where?', options: ['X', 'Y'], multiSelect: true },
  ])
  const path = `/api/sessions/s2/questions/${pending.id}`
  for (const [payload, code] of [
    [{ answers: [{ selected: ['A'] }] }, 'answers_mismatch'],
    [{ answers: [{ selected: ['A', 'B'] }, { selected: ['X'] }] }, 'single_select'],
    [{ answers: [{ selected: ['C'] }, { selected: ['X'] }] }, 'invalid_selection'],
    [{ answers: [{ selected: [] }, { selected: ['X'] }] }, 'answer_required'],
    [[], 'answers_mismatch'],
  ]) {
    const response = await post(handler, path, payload)
    assert.equal(response.status, 400, code)
    assert.equal(response.body.code, code)
  }
  assert.equal(questions.getPending('s2').length, 1)
  const skipped = await post(handler, path, { skip: true })
  assert.equal(skipped.status, 200)
  assert.deepEqual(await result, { outcome: 'skipped' })
  questions.dispose()
})

test('unknown or malformed question ids and wrong sessions are 404', async () => {
  const { questions, handler } = setup()
  const { pending } = await pendingQuestion(questions, 's3', [
    { question: 'Which?', options: ['A', 'B'] },
  ])
  for (const path of [
    '/api/sessions/s3/questions/not-a-question',
    '/api/sessions/s3/questions/q_00000000-0000',
    `/api/sessions/other/questions/${pending.id}`,
  ]) {
    const response = await post(handler, path, { skip: true })
    assert.equal(response.status, 404, path)
    assert.equal(response.body.code, 'question_not_found')
  }
  assert.equal(questions.getPending('s3').length, 1)
  questions.dispose()
})

// 问题结束了（运行停止、Runtime 重启）而前端没收到 question_resolved 时，问题卡只能靠 /live 清掉：
// 没有待回答的问题也必须带空数组，前端把缺字段当成旧 Runtime、沿用本地问题，卡片就一直关不掉。
test('/live always carries the pending question list so ended questions clear in the client', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-question-live-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.listStoredSessions = async () => []
  const id = 'session-question-live'
  runtime.sessions.set(id, {
    cwd: directory,
    name: '提问',
    session: {
      sessionId: id,
      isStreaming: false,
      model: { provider: 'openai', id: 'gpt-5.4' },
      messages: [],
      abort: async () => {},
      clearQueue: () => {},
      setActiveToolsByName: () => {},
      dispose: () => {},
    },
  })
  const controller = new AbortController()
  const result = runtime.questions.request({
    sessionId: id,
    questions: normalizeQuestions([{ question: 'Which?', options: ['A', 'B'] }]),
    signal: controller.signal,
  })
  const asked = await runtime.getSessionLive(id)
  assert.equal(asked.questions.length, 1)

  controller.abort()
  assert.equal((await result).outcome, 'cancelled')
  const ended = await runtime.getSessionLive(id)
  assert.deepEqual(ended.questions, [])
  const answer = runtime.answerSessionQuestion(id, asked.questions[0].id, { skip: true })
  assert.equal(answer.alreadyResolved, true)
})

// 问题已经结束（404 / 409）时前端移除问题卡并提示，而不是留着一张怎么点都失败的卡片（兼容旧 Runtime）。
test('answering an ended question removes the card instead of failing forever', async () => {
  const source = await readFile(
    new URL('../../src/features/chat/use-session-commands.ts', import.meta.url),
    'utf8',
  )
  const answer = source.slice(source.indexOf('const answerQuestion = useCallback'))
  assert.match(
    answer.slice(0, answer.indexOf('\n  )\n')),
    /error\.status === 404 \|\| error\.status === 409\)\) \{\s*removeQuestion\(\)\s*notify\(t\('board:question\.closed'\), 'info'\)\s*return/,
  )
})
