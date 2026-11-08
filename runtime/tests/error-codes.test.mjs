// 错误码只有十来个：Runtime 给英文 message，HTTP 带 { code, params }，前端两种语言都能翻译每个码和名词。
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import {
  ERROR_CODES,
  ERROR_SUBJECTS,
  PisperError,
  errorMessage,
  errorPayload,
  pisperError,
} from '../../shared/errors.mjs'

const locale = (language) => JSON.parse(readFileSync(`src/locales/${language}/errors.json`, 'utf8'))

test('PisperError carries an English message, a code, params and a status', () => {
  const error = pisperError('not_found', 'workflow')
  assert.ok(error instanceof PisperError)
  assert.equal(error.message, 'Workflow not found.')
  assert.equal(error.statusCode, 404)
  assert.deepEqual(errorPayload(error), { code: 'not_found', params: { subject: 'workflow' } })
  assert.equal(
    errorMessage('invalid', { subject: 'model', detail: 'use provider/modelId' }),
    'Model is invalid: use provider/modelId',
  )
  // 未知码按 failed 处理，未登记的名词原样显示。
  assert.equal(new PisperError('nope', { subject: 'gizmo' }).message, 'gizmo failed.')
  assert.deepEqual(errorPayload(new Error('x')), {})
})

test('the catalog stays small and both locales translate every code and subject', () => {
  assert.ok(Object.keys(ERROR_CODES).length <= 12, 'add a detail instead of another code')
  const en = locale('en-US')
  const zh = locale('zh-CN')
  assert.deepEqual(Object.keys(zh).sort(), Object.keys(en).sort())
  for (const code of Object.keys(ERROR_CODES)) {
    assert.equal(`${en[`code.${code}`]}.`, ERROR_CODES[code][1], code)
    assert.ok(zh[`code.${code}`], code)
  }
  for (const [subject, label] of Object.entries(ERROR_SUBJECTS)) {
    assert.equal(en[`subject.${subject}`], label, subject)
    assert.ok(zh[`subject.${subject}`], subject)
  }
  for (const key of Object.keys(en).filter((key) => key.startsWith('subject.')))
    assert.ok(Object.hasOwn(ERROR_SUBJECTS, key.slice('subject.'.length)), key)
})

test('the frontend shows Runtime errors in the interface language', async () => {
  const { i18n } = await import('../../src/app/i18n.ts')
  const { localizedRuntimeError } = await import('../../src/lib/runtime-error.ts')
  const payload = {
    error: 'Workflow not found.',
    code: 'not_found',
    params: { subject: 'workflow' },
  }
  const before = i18n.language
  try {
    await i18n.changeLanguage('zh-CN')
    assert.equal(localizedRuntimeError(payload, payload.error), '工作流不存在。')
    assert.equal(
      localizedRuntimeError({ code: 'busy', params: { subject: 'session' } }, ''),
      '会话正忙，请等它完成或先停止。',
    )
    assert.equal(
      localizedRuntimeError({ code: 'invalid', params: { subject: 'gizmo', detail: 'max 3' } }, ''),
      'gizmo无效：max 3',
    )
    // 没有码或不认识的码时原样显示 Runtime 的英文 message。
    assert.equal(localizedRuntimeError({ error: 'raw' }, 'raw'), 'raw')
    await i18n.changeLanguage('en-US')
    assert.equal(localizedRuntimeError(payload, ''), 'Workflow not found.')
  } finally {
    await i18n.changeLanguage(before)
  }
})
