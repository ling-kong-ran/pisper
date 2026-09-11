import assert from 'node:assert/strict'
import test from 'node:test'
import {
  LocalPathRevealError,
  LOCAL_PATH_REVEAL_TIMEOUT_MS,
  requestLocalPathReveal,
} from '../../src/lib/local-path-reveal.ts'

test('local reveal reports an unavailable Runtime instead of silently ignoring the click', async () => {
  await assert.rejects(requestLocalPathReveal('C:/report.txt', undefined), (error) => {
    assert.ok(error instanceof LocalPathRevealError)
    assert.equal(error.reason, 'unavailable')
    return true
  })
})

test('local reveal preserves Runtime rejections and synchronous errors', async () => {
  const syncError = new Error('Runtime is unavailable.')
  await assert.rejects(
    requestLocalPathReveal('C:/report.txt', () => {
      throw syncError
    }),
    (error) => error === syncError,
  )
  const rejection = 'Runtime local path reveal failed'
  await assert.rejects(
    requestLocalPathReveal('C:/report.txt', () => Promise.reject(rejection)),
    (error) => error === rejection,
  )
})

test('local reveal only reports acceptance for an explicit true result', async () => {
  const requested = []
  await requestLocalPathReveal('C:/报告 a.gif', async (path) => {
    requested.push(path)
    return true
  })
  assert.deepEqual(requested, ['C:/报告 a.gif'])
  await assert.rejects(
    requestLocalPathReveal('C:/report.txt', async () => false),
    (error) => error.reason === 'failed',
  )
})

test('an unreturned reveal times out without retrying or reporting late success', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  let complete
  let count = 0
  const request = requestLocalPathReveal('C:/report.txt', () => {
    count += 1
    return new Promise((resolve) => {
      complete = resolve
    })
  })
  const rejected = assert.rejects(request, (error) => error.reason === 'timeout')
  await Promise.resolve()
  t.mock.timers.tick(LOCAL_PATH_REVEAL_TIMEOUT_MS)
  await rejected
  complete(true)
  await Promise.resolve()
  assert.equal(count, 1)
})
