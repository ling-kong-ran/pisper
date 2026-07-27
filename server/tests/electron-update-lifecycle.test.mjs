import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createUpdateLogger, shutdownWithDeadline } from '../../electron/update-lifecycle.mjs'

test('desktop update logger persists updater events and error stacks', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-update-log-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const filePath = join(directory, 'updater.log')
  const logger = createUpdateLogger({ filePath, now: () => new Date('2026-07-22T00:00:00.000Z') })
  logger.info('Update available.', { version: '0.1.3' })
  logger.error('Install failed.', new Error('installer blocked'))
  const content = await readFile(filePath, 'utf8')
  assert.match(content, /\[INFO\].*Update available.*0\.1\.3/)
  assert.match(content, /\[ERROR\].*Install failed.*installer blocked/s)
})

test('desktop shutdown destroys the renderer and closes services before exiting', async () => {
  const events = []
  const result = await shutdownWithDeadline({
    destroy: () => { events.push('destroy') },
    close: async () => { events.push('close') },
    exit: (code) => { events.push(`exit:${code}`) },
    timeoutMs: 100,
  })
  assert.equal(result.timedOut, false)
  assert.deepEqual(events, ['destroy', 'close', 'exit:0'])
})

test('desktop shutdown forces exit when service cleanup stalls', async () => {
  const warnings = []
  let exitCode = null
  const result = await shutdownWithDeadline({
    destroy: () => {},
    close: () => new Promise(() => {}),
    exit: (code) => { exitCode = code },
    logger: { warn: (message) => warnings.push(message), error: () => {} },
    timeoutMs: 5,
  })
  assert.equal(result.timedOut, true)
  assert.equal(exitCode, 0)
  assert.match(warnings[0], /forcing exit/)
})

test('desktop shutdown can release resources before an updater starts without exiting early', async () => {
  const events = []
  const result = await shutdownWithDeadline({
    destroy: () => { events.push('destroy') },
    close: async () => { events.push('close') },
    timeoutMs: 100,
  })
  assert.equal(result.timedOut, false)
  assert.deepEqual(events, ['destroy', 'close'])
})
