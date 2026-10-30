import assert from 'node:assert/strict'
import test from 'node:test'
import { createApiHandler, requiredRuntimeFeature } from '../http/api-handler.mjs'
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
