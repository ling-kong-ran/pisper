import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtemp, readdir, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { PNG } from 'pngjs'
import { ImageAgentService } from '../services/image-agent-service.mjs'
import { ImageToolsPlugin } from '../services/image-tools-plugin.mjs'

const result = { output: { type: 'workflow-images', version: 1, frames: [] }, summary: 'done' }
const bytes = PNG.sync.write({ width: 2, height: 2, data: Buffer.alloc(16, 200) })
const reference = {
  id: 'reference',
  name: 'reference.png',
  mimeType: 'image/png',
  size: bytes.length,
}
const frame = {
  media: reference,
  width: 2,
  height: 2,
  action: 'walk',
  direction: 'S',
  columns: 1,
  rows: 1,
  frameCount: 1,
  durationMs: 125,
}
const atlas = {
  type: 'workflow-images',
  version: 1,
  frames: [frame],
  atlas: {
    media: reference,
    width: 2,
    height: 2,
    frames: [{ x: 0, y: 0, width: 2, height: 2, durationMs: 125, action: 'walk', direction: 'S' }],
  },
}
const enabled = () => new ImageToolsPlugin({ isAgentEnabled: () => true })
const media = () => ({
  upload: async () => reference,
  read: async () => ({ media: reference, buffer: bytes }),
})
async function workspace(t) {
  const cwd = await mkdtemp(join(tmpdir(), 'pisper-agent-lifecycle-'))
  t.after(() => rm(cwd, { recursive: true, force: true }))
  return cwd
}

test('agent service checks each execution against the current switch and retains consumer identity', async () => {
  let allowed = true,
    executions = 0
  const service = new ImageAgentService({
    plugin: new ImageToolsPlugin({ isAgentEnabled: () => allowed }),
    media: media(),
    operations: {
      execute: async () => {
        executions++
        return result
      },
    },
  })
  assert.equal(await service.execute({ operation: 'preview' }), result)
  allowed = false
  await assert.rejects(service.execute({ operation: 'preview', consumer: 'workbench' }), {
    code: 'image_tools_agent_disabled',
  })
  assert.equal(executions, 1)
  await service.dispose()
})

test('disposal aborts in-flight operations, blocks new work and never disposes borrowed dependencies', async () => {
  const entered = Promise.withResolvers()
  const operations = {
    execute: (request) => {
      entered.resolve(request.signal)
      return new Promise((_, reject) =>
        request.signal.addEventListener('abort', () => reject(request.signal.reason), {
          once: true,
        }),
      )
    },
    dispose: () => {
      throw new Error('Shared operations are owned by Runtime')
    },
  }
  const storage = {
    ...media(),
    dispose: () => {
      throw new Error('Media is owned by Runtime')
    },
  }
  const service = new ImageAgentService({ operations, media: storage, plugin: enabled() })
  const task = service.execute({ operation: 'preview' })
  const rejected = assert.rejects(task, { name: 'AbortError' })
  const signal = await entered.promise
  const closing = service.dispose()
  assert.equal(closing, service.dispose())
  assert.equal(signal.aborted, true)
  await assert.rejects(service.execute({ operation: 'preview' }), { code: 'image_tools_closed' })
  await assert.rejects(service.importImage('/workspace', 'image.png'), {
    code: 'image_tools_closed',
  })
  await assert.rejects(service.exportImages('/workspace', atlas), { code: 'image_tools_closed' })
  await rejected
  await closing
})

test('same-turn disposal prevents operations and authorization from starting', async () => {
  let called = false
  const service = new ImageAgentService({
    operations: {
      execute: async () => {
        called = true
        return result
      },
    },
    media: media(),
    plugin: {
      assertAllowed: async () => {
        called = true
      },
    },
  })
  const task = service.execute({ operation: 'preview' })
  const rejected = assert.rejects(task, { name: 'AbortError' })
  await service.dispose()
  await rejected
  assert.equal(called, false)
})

test('disposal waits for an already committing import before allowing media shutdown', async (t) => {
  const cwd = await workspace(t)
  await writeFile(join(cwd, 'reference.png'), bytes)
  const entered = Promise.withResolvers(),
    committed = Promise.withResolvers()
  let uploads = 0
  const service = new ImageAgentService({
    operations: { execute: async () => result },
    plugin: enabled(),
    media: {
      ...media(),
      upload: async () => {
        uploads++
        entered.resolve()
        await committed.promise
        return reference
      },
    },
  })
  const task = service.importImage(cwd, 'reference.png')
  await entered.promise
  let closed = false
  const closing = service.dispose().then(() => {
    closed = true
  })
  await Promise.resolve()
  assert.equal(closed, false)
  committed.resolve()
  assert.deepEqual(await task, reference)
  await closing
  assert.equal(closed, true)
  assert.equal(uploads, 1)
})

test('disposal waits for export reads and cancels before writing workspace files', async (t) => {
  const cwd = await workspace(t)
  const entered = Promise.withResolvers(),
    release = Promise.withResolvers()
  const service = new ImageAgentService({
    operations: { execute: async () => result },
    plugin: enabled(),
    media: {
      ...media(),
      read: async () => {
        entered.resolve()
        await release.promise
        return { media: reference, buffer: bytes }
      },
    },
  })
  const task = service.exportImages(cwd, atlas)
  const rejected = assert.rejects(task, { name: 'AbortError' })
  await entered.promise
  let closed = false
  const closing = service.dispose().then(() => {
    closed = true
  })
  await Promise.resolve()
  assert.equal(closed, false)
  release.resolve()
  await rejected
  await closing
  assert.equal(closed, true)
  assert.deepEqual(await readdir(cwd), [])
})

test('caller cancellation survives asynchronous authorization and does not poison following tasks', async () => {
  const entered = Promise.withResolvers(),
    release = Promise.withResolvers()
  let calls = 0
  const service = new ImageAgentService({
    operations: {
      execute: async () => {
        calls++
        return result
      },
    },
    media: media(),
    plugin: {
      assertAllowed: async () => {
        entered.resolve()
        await release.promise
      },
    },
  })
  const controller = new AbortController()
  const task = service.execute({ operation: 'preview', signal: controller.signal })
  const rejected = assert.rejects(task, { name: 'AbortError' })
  await entered.promise
  controller.abort()
  release.resolve()
  await rejected
  assert.equal(calls, 0)
  assert.deepEqual(await service.execute({ operation: 'preview' }), result)
  assert.equal(calls, 1)
  await service.dispose()
})
