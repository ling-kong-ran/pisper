// pi.dev 模型元数据匹配回归：网关前缀、矛盾条目取最大、后缀变体隔离。
import assert from 'node:assert/strict'
import { mkdtemp, readdir, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { PiDevModelMetadataService } from '../services/pi-dev-model-metadata.mjs'
import { writeJsonAtomic } from '../storage/json-file.mjs'

function serviceWith(models) {
  const service = new PiDevModelMetadataService({ cachePath: '/nonexistent/pisper-test.json' })
  service.buildIndex(new Map(Object.entries(models)))
  return service
}

test('dispose cancels response-body reads and prevents later background writes', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-pidev-dispose-'))
  const service = new PiDevModelMetadataService({ cachePath: join(directory, 'models.json') })
  t.after(async () => {
    await service.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  const reading = Promise.withResolvers()
  let signal
  let calls = 0
  t.mock.method(globalThis, 'fetch', async (_url, options) => {
    calls += 1
    signal = options.signal
    return {
      ok: true,
      text: () =>
        new Promise((_resolve, reject) => {
          signal.addEventListener('abort', () => reject(signal.reason), { once: true })
          reading.resolve()
        }),
    }
  })
  await service.init()
  await reading.promise
  await Promise.all([service.dispose(), service.dispose()])
  assert.equal(signal.aborted, true)
  assert.equal(service.fetching, null)
  await service.fetchOnceInBackground()
  assert.equal(calls, 1)
  assert.deepEqual(await readdir(directory), [])
})

test('dispose waits for owned persistence to settle', async () => {
  const service = serviceWith({})
  const pending = Promise.withResolvers()
  service.fetchAndPersist = () => pending.promise
  const task = service.fetchOnceInBackground()
  assert.equal(service.fetchOnceInBackground(), task)
  let disposed = false
  const closing = service.dispose().then(() => {
    disposed = true
  })
  await Promise.resolve()
  assert.equal(disposed, false)
  pending.resolve()
  await closing
  assert.equal(disposed, true)
  assert.equal(service.fetching, null)
})

test('prefixed gateway model ids resolve to the stored context window', () => {
  const service = serviceWith({
    'qwen3.8-max': { contextWindow: 1_000_000 },
    'alibaba/qwen3.8-max': { contextWindow: 1_000_000 },
  })
  // 线上事故形态：网关给模型 id 加各种分隔符的路由前缀，旧实现整串子串匹配失败，
  // 上下文窗口退回 200k 默认值。
  assert.equal(service.getContextWindowSync('aliyun_openai/qwen3.8-max'), 1_000_000)
  assert.equal(service.getContextWindowSync('xxx-qwen3.8-max'), 1_000_000)
  assert.equal(service.getContextWindowSync('XXX/QWEN3.8-MAX'), 1_000_000)
  assert.equal(service.getContextWindowSync('qwen3.8-max'), 1_000_000)
})

test('conflicting prefix variants resolve to the largest context window', () => {
  // pi.dev 真实数据形态：裸 gpt-5.4 记 272k，而 openai/gpt-5.4 是 1050k。
  // 用户裁定 1050000 才是最大上下文；与 models.dev 归一化一致按最大值取舍，
  // 也避免结果依赖 Map 迭代顺序。
  const service = serviceWith({
    'gpt-5.4': { contextWindow: 272_000 },
    'openai.gpt-5.4': { contextWindow: 272_000 },
    'openai/gpt-5.4': { contextWindow: 1_050_000 },
  })
  assert.equal(service.getContextWindowSync('gpt-5.4'), 1_050_000)
  assert.equal(service.getContextWindowSync('relay/gpt-5.4'), 1_050_000)
})

test('suffix variants never leak their window into the base model', () => {
  const service = serviceWith({
    'gpt-9': { contextWindow: 100_000 },
    'vendor/gpt-9-pro': { contextWindow: 999_000 },
    'alibaba/qwen3.8-max-0902': { contextWindow: 991_000 },
    'qwen3.8-max': { contextWindow: 1_000_000 },
  })
  // 尾部 token 对齐保证 -pro/-0902/-mini 变体的窗口不会串给基础型号，反之亦然。
  assert.equal(service.getContextWindowSync('gpt-9'), 100_000)
  assert.equal(service.getContextWindowSync('xxx/qwen3.8-max'), 1_000_000)
  assert.equal(service.getContextWindowSync('gpt-9-pro'), 999_000)
})

test('short stored ids match long gateway queries and misses return null', () => {
  const service = serviceWith({ k3: { contextWindow: 262_144 } })
  assert.equal(service.getContextWindowSync('kimi-for-coding/k3'), 262_144)
  // k3-256k 是另一个型号：尾部 token 不对齐，不得借用 k3 的窗口。
  assert.equal(service.getContextWindowSync('k3-256k'), null)
  assert.equal(service.getContextWindowSync('unknown-model'), null)
  assert.equal(service.getContextWindowSync(''), null)
  assert.equal(service.getContextWindowSync(null), null)
})

test('init loads persisted data; a missing file fetches once without retry storms', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-pidev-store-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const originalFetch = globalThis.fetch
  let fetchCalls = 0
  globalThis.fetch = async () => {
    fetchCalls += 1
    throw new Error('offline in test')
  }
  try {
    // 已有落盘数据：init 纯本地加载，不碰网络。
    const cachePath = join(directory, 'pi-dev-models.json')
    globalThis.fetch = originalFetch
    await writeJsonAtomic(cachePath, {
      models: { 'claude-opus-5': { contextWindow: 1_000_000 } },
    })
    globalThis.fetch = async () => {
      fetchCalls += 1
      throw new Error('offline in test')
    }
    const loaded = new PiDevModelMetadataService({ cachePath })
    await loaded.init()
    assert.equal(fetchCalls, 0)
    assert.equal(loaded.getContextWindowSync('anthropic/claude-opus-5'), 1_000_000)

    // 文件缺失：只做一次后台抓取；失败不抛出，查找保持为空由上层兜底。
    const missing = new PiDevModelMetadataService({ cachePath: join(directory, 'absent.json') })
    await missing.init()
    await missing.fetching
    assert.equal(fetchCalls, 1)
    assert.equal(missing.getContextWindowSync('claude-opus-5'), null)

    // 并发防重：同时触发多次也只多飞一次请求（每次启动一次，失败后下次启动自然重试）。
    const first = missing.fetchOnceInBackground()
    missing.fetchOnceInBackground()
    await first
    assert.equal(fetchCalls, 2)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('stale pi.dev data is refetched when idle, with a retry backoff after failures', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-pidev-stale-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const cachePath = join(directory, 'pi-dev-models.json')
  const page = (id, context) =>
    `<table><tr data-model-row="true" data-model-name="${id}" data-model-id="${id}"><td data-label="Context">${context}</td></tr></table>`
  let html = page('glm-5.3', '1,048,576')
  let fail = false
  let calls = 0
  t.mock.method(globalThis, 'fetch', async () => {
    calls += 1
    if (fail) throw new Error('offline in test')
    return { ok: true, status: 200, text: async () => html }
  })

  // 旧版本落盘的文件没有抓取时间：视为过期，空闲时重抓，查得到新模型。
  await writeJsonAtomic(cachePath, { models: { 'glm-5': { contextWindow: 200_000 } } })
  const service = new PiDevModelMetadataService({ cachePath })
  t.after(() => service.dispose())
  await service.init()
  assert.equal(calls, 0)
  assert.equal(service.getContextWindowSync('glm-5.3'), null)
  const day = 24 * 60 * 60 * 1000
  await service.refreshIfStale(7 * day)
  assert.equal(calls, 1)
  assert.equal(service.getContextWindowSync('glm-5.3'), 1_048_576)
  const stored = JSON.parse(await readFile(cachePath, 'utf8'))
  assert.ok(Date.parse(stored.fetchedAt) > 0)

  // 新鲜数据不重抓；重新加载后仍认得抓取时间。
  assert.equal(service.refreshIfStale(7 * day), null)
  const reloaded = new PiDevModelMetadataService({ cachePath })
  t.after(() => reloaded.dispose())
  await reloaded.init()
  assert.equal(reloaded.refreshIfStale(7 * day), null)
  assert.equal(calls, 1)

  // 过期后抓取失败：保留旧数据，一小时内不再重试。
  fail = true
  const later = Date.now() + 8 * day
  await reloaded.refreshIfStale(7 * day, { now: later })
  assert.equal(calls, 2)
  assert.equal(reloaded.getContextWindowSync('glm-5.3'), 1_048_576)
  assert.equal(reloaded.refreshIfStale(7 * day, { now: later }), null)
  assert.equal(calls, 2)
})
