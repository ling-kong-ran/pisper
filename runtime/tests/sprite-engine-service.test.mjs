import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdtemp, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { SpriteEngineService } from '../services/sprite-engine-service.mjs'
import {
  parseSpriteEngineCatalog,
  SPRITE_ENGINE_CATALOG,
} from '../../shared/sprite-engine-catalog.mjs'

const CONTENTS = {
  'runtime.js': Buffer.from('/* pinned test runtime */'),
  'model.onnx': Buffer.from('pinned test model'),
  'LICENSE.txt': Buffer.from('test resource license'),
}
const DEFINITION = {
  id: 'background',
  name: 'Test engine',
  version: '1.0.0',
  licenses: [],
  files: Object.entries(CONTENTS).map(([name, bytes]) => ({
    name,
    bytes: bytes.length,
    sha256: createHash('sha256').update(bytes).digest('hex'),
    url: `https://fixtures.invalid/${name}`,
    mimeType: name.endsWith('.js') ? 'text/javascript' : 'application/octet-stream',
  })),
}
const bundleFiles = () =>
  Object.fromEntries(
    Object.entries(CONTENTS).map(([name, bytes]) => [`engines/background/${name}`, bytes]),
  )
const successfulFetch = async (url) => new Response(CONTENTS[new URL(url).pathname.slice(1)])

async function fixture(t, options = {}) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-sprite-engine-'))
  const service = new SpriteEngineService({
    dataDir,
    definitions: [DEFINITION],
    fetchFn: successfulFetch,
    ...options,
  })
  t.after(async () => {
    await service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  await service.init()
  return { service, dataDir }
}

async function finished(service) {
  await service.active.get('background')?.done
  return (await service.catalog()).engines[0]
}

test('engine initialization never downloads; explicit download verifies and durably installs static resources', async (t) => {
  let requests = 0
  const { service, dataDir } = await fixture(t, {
    fetchFn: async (url) => {
      requests++
      return successfulFetch(url)
    },
  })
  assert.equal(requests, 0)
  assert.equal(parseSpriteEngineCatalog(await service.catalog()).engines[0].status, 'missing')
  const downloading = await service.download('background')
  assert.equal(downloading.engines[0].status, 'downloading')
  const ready = await finished(service)
  assert.equal(ready.status, 'ready')
  assert.equal(ready.received, ready.total)
  assert.equal(requests, 3)
  assert.deepEqual((await service.file('background', 'runtime.js')).buffer, CONTENTS['runtime.js'])
  await service.download('background')
  assert.equal(requests, 3)
  const exported = await service.exportFiles()
  assert.deepEqual(exported, bundleFiles())
  await service.dispose()
  const restarted = new SpriteEngineService({
    dataDir,
    definitions: [DEFINITION],
    fetchFn: () => {
      throw new Error('unexpected network')
    },
  })
  try {
    assert.equal((await restarted.catalog()).engines[0].status, 'ready')
    assert.deepEqual(
      (await restarted.file('background', 'model.onnx')).buffer,
      CONTENTS['model.onnx'],
    )
    assert.equal((await restarted.remove('background')).engines[0].status, 'missing')
  } finally {
    await restarted.dispose()
  }
})

test('download rejects oversized and mismatched payloads without publishing a partial engine', async (t) => {
  let response = () => new Response(Buffer.from('forged'))
  const { service, dataDir } = await fixture(t, { fetchFn: async () => response() })
  await service.download('background')
  assert.equal((await finished(service)).error, 'sprite_engine_integrity')
  response = () => new Response(Buffer.alloc(200), { headers: { 'content-length': '200' } })
  await service.download('background')
  assert.equal((await finished(service)).status, 'failed')
  await assert.rejects(service.file('background', 'runtime.js'), { code: 'sprite_engine_missing' })
  assert.ok(
    !(await readdir(join(dataDir, 'sprite-engines', 'background'))).some((name) =>
      name.startsWith('staging-'),
    ),
  )
  assert.deepEqual(await service.exportFiles(), {})
})

test('download switches pinned sources after network and checksum failure and reports the failed file', async (t) => {
  const definition = {
    ...DEFINITION,
    files: DEFINITION.files.map((file) => ({
      ...file,
      fallbackUrls: [`https://mirror.invalid/${file.name}`],
    })),
  }
  const requests = []
  const { service } = await fixture(t, {
    definitions: [definition],
    fetchFn: async (url) => {
      requests.push(String(url))
      if (new URL(url).hostname === 'fixtures.invalid') {
        if (String(url).endsWith('runtime.js')) throw new TypeError('Network fixture')
        return new Response(Buffer.from('invalid file'))
      }
      return successfulFetch(url)
    },
  })
  await service.download('background')
  assert.equal((await finished(service)).status, 'ready')
  assert.equal(requests.length, 6)
  assert.equal(service.state('background').received, service.state('background').total)
  await service.remove('background')
  service.fetchFn = async () => new Response('unavailable', { status: 503 })
  await service.download('background')
  const failed = await finished(service)
  assert.equal(failed.status, 'failed')
  assert.equal(failed.file, 'runtime.js')
  assert.equal(failed.error, 'sprite_engine_download_failed')
})

test('source timeout switches to a verified mirror while user cancellation never starts a mirror request', async (t) => {
  const definition = {
    ...DEFINITION,
    files: DEFINITION.files
      .slice(0, 1)
      .map((file) => ({ ...file, fallbackUrls: [`https://mirror.invalid/${file.name}`] })),
  }
  const requests = []
  const waiting = Promise.withResolvers()
  const { service } = await fixture(t, {
    definitions: [definition],
    sourceTimeoutMs: 15,
    fetchFn: (url, { signal }) => {
      requests.push(String(url))
      if (new URL(url).hostname === 'mirror.invalid') return successfulFetch(url)
      waiting.resolve()
      return new Promise((_, reject) =>
        signal.addEventListener('abort', () => reject(signal.reason), { once: true }),
      )
    },
  })
  await service.download('background')
  assert.equal((await finished(service)).status, 'ready')
  assert.equal(requests.length, 2)
  await service.remove('background')
  requests.length = 0
  service.sourceTimeoutMs = 60000
  const cancelled = Promise.withResolvers()
  service.fetchFn = (url, { signal }) => {
    requests.push(String(url))
    cancelled.resolve()
    return new Promise((_, reject) =>
      signal.addEventListener('abort', () => reject(signal.reason), { once: true }),
    )
  }
  await service.download('background')
  await cancelled.promise
  await service.cancel('background')
  assert.equal(requests.length, 1)
  assert.equal(service.state('background').status, 'missing')
})

test('cancellation interrupts a streamed resource after bounded progress and releases download work', async (t) => {
  const reading = Promise.withResolvers()
  const bytes = CONTENTS['runtime.js']
  const { service } = await fixture(t, {
    fetchFn: async (_url, { signal }) => {
      let sent = false
      return new Response(
        new ReadableStream(
          {
            start(controller) {
              signal.addEventListener('abort', () => controller.error(new Error('aborted')), {
                once: true,
              })
            },
            pull(controller) {
              if (!sent) {
                sent = true
                controller.enqueue(bytes.subarray(0, 4))
                return
              }
              reading.resolve()
              return new Promise(() => {})
            },
          },
          { highWaterMark: 0 },
        ),
      )
    },
  })
  await service.download('background')
  await reading.promise
  assert.equal((await service.catalog()).engines[0].received, 4)
  const cancelled = await service.cancel('background')
  assert.equal(cancelled.engines[0].status, 'missing')
  assert.equal(cancelled.engines[0].received, 0)
  assert.equal(service.active.size, 0)
})

test('download timeout and shutdown abort fetches and await their cleanup', async (t) => {
  const fetching = Promise.withResolvers()
  const blockedFetch = async (_url, { signal }) => {
    fetching.resolve()
    return new Promise((_resolve, reject) => {
      if (signal.aborted) reject(new Error('aborted'))
      else signal.addEventListener('abort', () => reject(new Error('aborted')), { once: true })
    })
  }
  const timed = await fixture(t, { fetchFn: blockedFetch, timeoutMs: 5 })
  await timed.service.download('background')
  assert.equal((await finished(timed.service)).error, 'sprite_engine_timeout')
  const closing = await fixture(t, { fetchFn: blockedFetch })
  await closing.service.download('background')
  await fetching.promise
  await closing.service.dispose()
  assert.equal(closing.service.active.size, 0)
  assert.equal((await closing.service.catalog()).engines[0].status, 'missing')
})

test('offline engine import accepts only complete catalog-pinned suites including licenses', async (t) => {
  const { service } = await fixture(t, {
    fetchFn: () => {
      throw new Error('unexpected network')
    },
  })
  const files = bundleFiles()
  service.validateBundleFiles(files)
  assert.equal((await service.installBundleFiles(files)).engines[0].status, 'ready')
  assert.deepEqual(await service.exportFiles(), files)
  const incomplete = { ...files }
  delete incomplete['engines/background/LICENSE.txt']
  assert.throws(() => service.validateBundleFiles(incomplete), {
    code: 'sprite_engine_bundle_incomplete',
  })
  assert.throws(
    () =>
      service.validateBundleFiles({
        ...files,
        'engines/background/execute.sh': Buffer.from('bad'),
      }),
    { code: 'sprite_engine_bundle_invalid' },
  )
  assert.throws(
    () => service.validateBundleFiles({ ...files, 'engines/../runtime.js': Buffer.from('bad') }),
    { code: 'sprite_engine_bundle_invalid' },
  )
  assert.throws(
    () =>
      service.installBundleFiles({
        ...files,
        'engines/background/runtime.js': Buffer.from('changed'),
      }),
    { code: 'sprite_engine_integrity' },
  )
  assert.equal((await service.catalog()).engines[0].status, 'ready')
})

test('serving cached resources rechecks the checksum and rejects names or symlinks outside the catalog', async (t) => {
  const { service, dataDir } = await fixture(t)
  await service.installBundleFiles(bundleFiles())
  assert.throws(() => service.file('background', '../runtime.js'), {
    code: 'sprite_engine_file_not_found',
  })
  assert.throws(() => service.file('outside', 'runtime.js'), { code: 'sprite_engine_not_found' })
  const root = join(dataDir, 'sprite-engines', 'background')
  const installed = JSON.parse(await readFile(join(root, 'installed.json'), 'utf8'))
  const path = join(root, installed.directory, 'runtime.js')
  await writeFile(path, Buffer.alloc(CONTENTS['runtime.js'].length))
  await assert.rejects(service.file('background', 'runtime.js'), {
    code: 'sprite_engine_integrity',
  })
  await rm(path)
  const outside = join(dataDir, 'outside.js')
  await writeFile(outside, CONTENTS['runtime.js'])
  await symlink(outside, path)
  await assert.rejects(service.file('background', 'runtime.js'), {
    code: 'sprite_engine_integrity',
  })
})

test('production catalog pins every executable, model and required license resource', () => {
  assert.deepEqual(
    SPRITE_ENGINE_CATALOG.map(({ id }) => id),
    ['background', 'inpaint'],
  )
  for (const engine of SPRITE_ENGINE_CATALOG) {
    assert.equal(engine.files.filter(({ name }) => name.startsWith('LICENSE-')).length, 2)
    for (const file of engine.files) {
      assert.match(file.sha256, /^[a-f0-9]{64}$/)
      assert.ok(file.bytes > 0)
      assert.equal(new URL(file.url).protocol, 'https:')
      for (const url of [file.url, ...(file.fallbackUrls ?? [])])
        assert.ok(
          ['cdn.jsdelivr.net', 'gh-proxy.com', 'ghfast.top'].includes(new URL(url).hostname),
          'Algorithm downloads must use a pinned CDN, never a direct GitHub source',
        )
    }
  }
  assert.throws(() => parseSpriteEngineCatalog({ engines: [{ id: 'unknown' }] }))
})
