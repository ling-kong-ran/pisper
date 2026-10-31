// 代理的提问（ask_user，规格 v2 第 8.4 节）在前端的解析与点选规则：Runtime 发来的待回答问题按不可信输入解析、限长；
// 全部代理行内点选只给一题单选、选项 ≤4 的提问；全部问题都有答案才能提交。
import assert from 'node:assert/strict'
import test from 'node:test'
import {
  answersComplete,
  inlineQuestionOptions,
  parsePendingQuestion,
  parsePendingQuestions,
} from '../../src/features/chat/session-questions.ts'

const question = (overrides = {}) => ({
  id: 'q_0123456789ab',
  createdAt: '2026-10-31T08:00:00.000Z',
  questions: [
    {
      header: '数据库',
      question: '用哪个数据库？',
      options: [{ label: 'Postgres', description: '已有连接' }, { label: 'SQLite' }],
      multiSelect: false,
    },
  ],
  ...overrides,
})

test('pending questions parse defensively and drop malformed or duplicate entries', () => {
  const parsed = parsePendingQuestion(question())
  assert.deepEqual(parsed, {
    id: 'q_0123456789ab',
    createdAt: '2026-10-31T08:00:00.000Z',
    questions: [
      {
        header: '数据库',
        question: '用哪个数据库？',
        options: [
          { label: 'Postgres', description: '已有连接' },
          { label: 'SQLite', description: '' },
        ],
        multiSelect: false,
      },
    ],
  })
  // 会话摘要里的 question 带 questionId 也认；非法 id、少于 2 个选项、空问题都丢掉。
  assert.equal(
    parsePendingQuestion({ ...question(), id: undefined, questionId: 'q_0123456789ab' })?.id,
    'q_0123456789ab',
  )
  assert.equal(parsePendingQuestion({ ...question(), id: 'bad id' }), null)
  assert.equal(
    parsePendingQuestion(
      question({ questions: [{ question: 'x', options: [{ label: 'only' }] }] }),
    ),
    null,
  )
  assert.equal(parsePendingQuestion(question({ questions: [] })), null)
  assert.equal(parsePendingQuestion('junk'), null)
  // 超长字段截断，重复选项去重，最多 6 个选项、4 个问题。
  const long = parsePendingQuestion(
    question({
      questions: [
        {
          header: 'h'.repeat(40),
          question: 'q'.repeat(500),
          options: [...Array.from({ length: 9 }, (_, i) => ({ label: `o${i}` })), { label: 'o1' }],
        },
      ],
    }),
  )
  assert.equal(long.questions[0].header.length, 24)
  assert.equal(long.questions[0].question.length, 400)
  assert.equal(long.questions[0].options.length, 6)
  const list = parsePendingQuestions([
    question({ id: 'q_bbbbbbbbbbbb', createdAt: '2026-10-31T09:00:00.000Z' }),
    question(),
    question(),
    null,
  ])
  assert.deepEqual(
    list.map((item) => item.id),
    ['q_0123456789ab', 'q_bbbbbbbbbbbb'],
  )
  assert.deepEqual(parsePendingQuestions('junk'), [])
})

test('inline answering only covers one single-select question with up to four options', () => {
  assert.deepEqual(inlineQuestionOptions(parsePendingQuestion(question())), ['Postgres', 'SQLite'])
  const multi = parsePendingQuestion(
    question({
      questions: [{ ...question().questions[0], multiSelect: true }],
    }),
  )
  assert.equal(inlineQuestionOptions(multi), null)
  const five = parsePendingQuestion(
    question({
      questions: [
        {
          question: '哪一个？',
          options: ['a', 'b', 'c', 'd', 'e'].map((label) => ({ label })),
        },
      ],
    }),
  )
  assert.equal(inlineQuestionOptions(five), null)
  const two = parsePendingQuestion(
    question({ questions: [question().questions[0], question().questions[0]] }),
  )
  assert.equal(inlineQuestionOptions(two), null)
  assert.equal(inlineQuestionOptions(null), null)
})

test('answers are complete only when every question has a choice or free text', () => {
  const two = parsePendingQuestion(
    question({ questions: [question().questions[0], question().questions[0]] }),
  )
  assert.equal(answersComplete(two, [{ selected: ['SQLite'] }, { selected: [] }]), false)
  assert.equal(
    answersComplete(two, [{ selected: ['SQLite'] }, { selected: [], other: '  ' }]),
    false,
  )
  assert.equal(
    answersComplete(two, [{ selected: ['SQLite'] }, { selected: [], other: 'MySQL' }]),
    true,
  )
})
