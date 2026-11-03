import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import {
  BETA_FEATURES,
  MAX_STORED_BETA_FLAGS,
  applyBetaFlagPatch,
  isBetaFeatureActive,
  normalizeStoredBetaFlags,
  projectBetaFeatures,
  validateBetaFeatureCatalog,
} from '../../shared/beta-features.mjs'
import { createApiHandler, requiredRuntimeFeature } from '../http/api-handler.mjs'
import { resolveRuntimeCapabilities } from '../runtime-capabilities.mjs'
import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'

const root = fileURLToPath(new URL('../../', import.meta.url))

// 测试用清单：正式清单在 L3 之前为空，读写与投影都用注入的清单验证。
const CATALOG = Object.freeze([
  {
    id: 'bots',
    stage: 'experimental',
    requires: ['bots'],
    feedbackUrl: 'https://github.com/example/pisper/issues/1',
  },
  {
    id: 'quiet-mode',
    stage: 'preview',
    requires: [],
    feedbackUrl: 'https://github.com/example/pisper/issues/2',
  },
])

test('the shipped beta catalog is valid and every feature has copy in both locales', async () => {
  validateBetaFeatureCatalog(BETA_FEATURES)
  if (BETA_FEATURES.length === 0) return
  for (const locale of ['zh-CN', 'en-US']) {
    const messages = JSON.parse(
      await readFile(join(root, 'src/locales', locale, 'labs.json'), 'utf8'),
    )
    for (const { id } of BETA_FEATURES) {
      for (const key of ['title', 'description', 'whenOff']) {
        assert.equal(
          typeof messages[`feature.${id}.${key}`],
          'string',
          `${locale} feature.${id}.${key}`,
        )
      }
    }
  }
})

test('catalog validation rejects bad ids, duplicates, stages and feedback links', () => {
  validateBetaFeatureCatalog(CATALOG)
  const base = CATALOG[0]
  assert.throws(() => validateBetaFeatureCatalog([{ ...base, id: 'Bots' }]), /id 不合法/)
  assert.throws(() => validateBetaFeatureCatalog([base, base]), /id 重复/)
  assert.throws(() => validateBetaFeatureCatalog([{ ...base, stage: 'alpha' }]), /阶段不合法/)
  assert.throws(
    () => validateBetaFeatureCatalog([{ ...base, feedbackUrl: 'http://example.com' }]),
    /反馈链接/,
  )
})

test('stored flags keep only well-formed booleans, including ids no longer in the catalog', () => {
  assert.deepEqual(
    normalizeStoredBetaFlags({
      bots: true,
      'retired-feature': false,
      'Bad Id': true,
      count: 1,
      nested: { on: true },
    }),
    { bots: true, 'retired-feature': false },
  )
  assert.deepEqual(normalizeStoredBetaFlags(null), {})
  assert.deepEqual(normalizeStoredBetaFlags(['bots']), {})
  const flood = Object.fromEntries(
    Array.from({ length: MAX_STORED_BETA_FLAGS + 10 }, (_, index) => [`f${index}`, true]),
  )
  assert.equal(Object.keys(normalizeStoredBetaFlags(flood)).length, MAX_STORED_BETA_FLAGS)
})

test('flag patches accept only catalog ids with boolean values and keep unknown stored keys', () => {
  const stored = { 'retired-feature': true }
  assert.deepEqual(applyBetaFlagPatch(stored, { bots: true }, CATALOG), {
    'retired-feature': true,
    bots: true,
  })
  assert.throws(() => applyBetaFlagPatch(stored, null, CATALOG), /JSON 对象/)
  assert.throws(() => applyBetaFlagPatch(stored, ['bots'], CATALOG), /JSON 对象/)
  assert.throws(() => applyBetaFlagPatch(stored, {}, CATALOG), /没有要修改/)
  assert.throws(() => applyBetaFlagPatch(stored, { 'retired-feature': false }, CATALOG), /没有这个/)
  assert.throws(() => applyBetaFlagPatch(stored, { bots: 'true' }, CATALOG), /布尔值/)
  // 任一项非法则整体拒绝，不做部分写入。
  assert.throws(() => applyBetaFlagPatch(stored, { bots: true, nope: true }, CATALOG), /没有这个/)
})

test('availability depends only on runtime capabilities; activity needs both flag and support', () => {
  const flags = { bots: true, 'quiet-mode': false }
  assert.deepEqual(projectBetaFeatures(flags, { bots: false }, CATALOG), [
    {
      id: 'bots',
      stage: 'experimental',
      feedbackUrl: 'https://github.com/example/pisper/issues/1',
      enabled: true,
      available: false,
      unavailableReason: 'runtime',
    },
    {
      id: 'quiet-mode',
      stage: 'preview',
      feedbackUrl: 'https://github.com/example/pisper/issues/2',
      enabled: false,
      available: true,
      unavailableReason: null,
    },
  ])
  assert.equal(isBetaFeatureActive(flags, { bots: false }, 'bots', CATALOG), false)
  assert.equal(isBetaFeatureActive(flags, undefined, 'bots', CATALOG), false)
  assert.equal(isBetaFeatureActive(flags, { bots: true }, 'bots', CATALOG), true)
  assert.equal(isBetaFeatureActive(flags, { bots: true }, 'quiet-mode', CATALOG), false)
  assert.equal(isBetaFeatureActive({ unknown: true }, {}, 'unknown', CATALOG), false)
})

test('every runtime profile exposes the beta feature settings route', async () => {
  assert.equal(requiredRuntimeFeature('/api/settings/beta-features', 'PATCH'), 'betaFeatures')
  for (const profile of ['desktop', 'mobile-embedded', 'mobile-store']) {
    const capabilities = await resolveRuntimeCapabilities({
      environment: { PISPER_RUNTIME_PROFILE: profile },
      moduleSupport: { childProcess: false, workerThreads: false, sqlite: false, wasm: false },
    })
    assert.equal(capabilities.features.betaFeatures, true, profile)
  }
})

async function facadeFixture(t, appConfig) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-beta-features-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const appConfigPath = join(directory, 'pisper.json')
  await writeFile(appConfigPath, JSON.stringify(appConfig))
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  Object.assign(runtime, {
    appConfigPath,
    capabilities: { features: { bots: true } },
    betaFeatureCatalog: CATALOG,
    betaFeatureFlags: normalizeStoredBetaFlags(appConfig.betaFeatures),
    betaFeatureWrite: Promise.resolve(),
  })
  return { runtime, appConfigPath }
}

test('runtime persists flag changes, keeps other config and retired keys, and applies them at once', async (t) => {
  const { runtime, appConfigPath } = await facadeFixture(t, {
    memoryAutoApproveConfidence: 70,
    betaFeatures: { 'retired-feature': true },
  })
  assert.equal(runtime.isBetaFeatureEnabled('bots'), false)

  const result = await runtime.updateBetaFeatures({ bots: true })
  assert.equal(result.features.find((feature) => feature.id === 'bots').enabled, true)
  assert.equal(runtime.isBetaFeatureEnabled('bots'), true)
  assert.deepEqual(JSON.parse(await readFile(appConfigPath, 'utf8')), {
    memoryAutoApproveConfidence: 70,
    betaFeatures: { 'retired-feature': true, bots: true },
  })

  await assert.rejects(runtime.updateBetaFeatures({ bots: 1 }), /布尔值/)
  assert.equal(runtime.isBetaFeatureEnabled('bots'), true)
})

test('concurrent flag changes are serialized so neither update is lost', async (t) => {
  const { runtime, appConfigPath } = await facadeFixture(t, {})
  await Promise.all([
    runtime.updateBetaFeatures({ bots: true }),
    runtime.updateBetaFeatures({ 'quiet-mode': true }),
  ])
  assert.deepEqual(JSON.parse(await readFile(appConfigPath, 'utf8')).betaFeatures, {
    bots: true,
    'quiet-mode': true,
  })
  assert.deepEqual(runtime.betaFeatureFlags, { bots: true, 'quiet-mode': true })
})

function request(method, body) {
  return {
    method,
    async *[Symbol.asyncIterator]() {
      if (body !== undefined) yield Buffer.from(JSON.stringify(body))
    },
  }
}

function response() {
  return {
    status: 0,
    body: '',
    destroyed: false,
    writableEnded: false,
    headersSent: false,
    writeHead(status) {
      this.status = status
    },
    flushHeaders() {},
    write(body = '') {
      this.body += body
    },
    end(body = '') {
      this.body += body
      this.writableEnded = true
    },
  }
}

test('beta feature API reads, updates, rejects bad input and honours the capability gate', async () => {
  const calls = []
  const state = { features: [] }
  const runtime = {
    capabilities: { features: { betaFeatures: true } },
    getBetaFeatures() {
      calls.push(['get'])
      return state
    },
    async updateBetaFeatures(input) {
      calls.push(['update', input])
      if (input.bots !== true) throw new Error('实验功能 bots 的开关必须是布尔值。')
      return state
    },
  }
  const handler = createApiHandler(runtime)
  const url = new URL('http://localhost/api/settings/beta-features')

  const getResponse = response()
  assert.equal(await handler(request('GET'), getResponse, url), true)
  assert.equal(getResponse.status, 200)
  assert.deepEqual(JSON.parse(getResponse.body), state)

  const patchResponse = response()
  await handler(request('PATCH', { bots: true }), patchResponse, url)
  assert.equal(patchResponse.status, 200)

  const badResponse = response()
  await handler(request('PATCH', { bots: 'yes' }), badResponse, url)
  assert.equal(badResponse.status, 400)
  assert.match(JSON.parse(badResponse.body).error, /布尔值/)
  assert.deepEqual(calls, [['get'], ['update', { bots: true }], ['update', { bots: 'yes' }]])

  const gated = createApiHandler({ capabilities: { features: { betaFeatures: false } } })
  const gatedResponse = response()
  await gated(request('GET'), gatedResponse, url)
  assert.equal(gatedResponse.status, 409)
  assert.equal(JSON.parse(gatedResponse.body).capability, 'betaFeatures')
})
