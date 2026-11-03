import assert from 'node:assert/strict'
import test from 'node:test'
import {
  finishRunStartTiming,
  markRunStage,
  measureRunStage,
  withRunStartTiming,
} from '../runtime/run-start-timing.mjs'

function captureStderr(t) {
  const lines = []
  const write = process.stderr.write
  process.stderr.write = (chunk, ...rest) => {
    if (String(chunk).startsWith('PISPER_RUN_START ')) {
      lines.push(String(chunk))
      return true
    }
    return write.call(process.stderr, chunk, ...rest)
  }
  t.after(() => (process.stderr.write = write))
  return lines
}

test('run start timing is silent unless PISPER_RUN_TIMING is set', async (t) => {
  const lines = captureStderr(t)
  const previous = process.env.PISPER_RUN_TIMING
  delete process.env.PISPER_RUN_TIMING
  t.after(() => {
    if (previous === undefined) delete process.env.PISPER_RUN_TIMING
    else process.env.PISPER_RUN_TIMING = previous
  })
  const result = await withRunStartTiming(async () => {
    markRunStage('session')
    finishRunStartTiming()
    return 'done'
  })
  assert.equal(result, 'done')
  assert.deepEqual(lines, [])
})

test('run start timing reports stage durations once, across awaits, without session content', async (t) => {
  const lines = captureStderr(t)
  const previous = process.env.PISPER_RUN_TIMING
  process.env.PISPER_RUN_TIMING = '1'
  t.after(() => {
    if (previous === undefined) delete process.env.PISPER_RUN_TIMING
    else process.env.PISPER_RUN_TIMING = previous
  })
  await withRunStartTiming(async () => {
    await new Promise((resolve) => setTimeout(resolve, 5))
    markRunStage('session')
    await measureRunStage('usage', new Promise((resolve) => setTimeout(resolve, 5)))
    markRunStage('usage')
    finishRunStartTiming('live')
    // 建立 live 之后的阶段不再记录，也不重复输出。
    markRunStage('later')
    finishRunStartTiming('ended')
  })
  assert.equal(lines.length, 1)
  const report = JSON.parse(lines[0].slice('PISPER_RUN_START '.length))
  assert.deepEqual(Object.keys(report.stages), ['session', 'usage', 'live'])
  assert.ok(report.stages.session >= 4)
  assert.ok(report.parallel.usage >= 4)
  assert.ok(report.totalMs >= report.stages.session)
})
