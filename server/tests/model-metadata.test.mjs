import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  BUNDLED_MODEL_METADATA,
  ModelMetadataService,
} from '../services/model-metadata-service.mjs'

function response(payload, { status = 200 } = {}) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: () => null },
    text: async () => JSON.stringify(payload),
  }
}

test('bundled model metadata resolves without network access', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-model-metadata-bundled-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  let calls = 0
  const metadata = new ModelMetadataService({
    path: join(directory, 'metadata.json'),
    fetchImpl: async () => {
      calls += 1
      throw new Error('unexpected request')
    },
  })
  await metadata.init()

  const expected = {
    'gpt-5.3-codex-spark': [128_000, 32_000],
    'gpt-5.4': [272_000, 128_000],
    'gpt-5.4-mini': [400_000, 128_000],
    'gpt-5.5': [272_000, 128_000],
    'gpt-5.6-terra': [272_000, undefined],
    'claude-fable-5': [1_000_000, 128_000],
    'claude-sonnet-4-6': [1_000_000, 128_000],
    'claude-opus-4-5': [200_000, 64_000],
    'glm-5.2': [1_000_000, 131_072],
    'glm-4.7': [204_800, 131_072],
    'deepseek-chat': [128_000, undefined],
    'deepseek-v4-pro': [1_000_000, 384_000],
    k2p7: [262_144, 32_768],
    k3: [1_048_576, 131_072],
    'kimi-k3': [1_048_576, 131_072],
    'grok-4.5': [500_000, 500_000],
    'gemini-2.5-pro': [1_048_576, 65_536],
    'gemini-3.1-pro-preview': [1_048_576, 65_536],
  }
  for (const [modelId, [contextWindow, maxTokens]] of Object.entries(expected)) {
    const model = await metadata.ensure(modelId)
    assert.equal(model.contextWindow, contextWindow, modelId)
    assert.equal(model.maxTokens, maxTokens, modelId)
  }
  const gpt56 = await metadata.ensure('gpt-5.6-sol')
  assert.deepEqual(gpt56.input, ['text', 'image'])
  assert.deepEqual(gpt56.thinkingLevelMap, { off: 'none', xhigh: 'xhigh', max: 'max' })
  for (const modelId of ['tokenhub/glm-5.2', 'tokenhub&glm-5.2', 'tokenhub%glm-5.2']) {
    const namespacedGlm = await metadata.ensure(modelId)
    assert.equal(namespacedGlm.contextWindow, 1_000_000, modelId)
    assert.equal(namespacedGlm.maxTokens, 131_072, modelId)
  }
  assert.equal(metadata.get('tokenhubglm-5.2'), null)
  assert.equal(calls, 0)
})

test('every bundled model declares its reasoning and extended thinking boundaries', () => {
  for (const [modelId, model] of Object.entries(BUNDLED_MODEL_METADATA)) {
    assert.equal(typeof model.reasoning, 'boolean', `${modelId} reasoning`)
    assert.equal(typeof model.thinkingLevelMap, 'object', `${modelId} thinkingLevelMap`)
    assert.equal(Object.hasOwn(model.thinkingLevelMap, 'xhigh'), true, `${modelId} xhigh boundary`)
    assert.equal(Object.hasOwn(model.thinkingLevelMap, 'max'), true, `${modelId} max boundary`)
  }

  assert.deepEqual(BUNDLED_MODEL_METADATA['glm-5.2'].thinkingLevelMap, {
    xhigh: null,
    max: 'max',
    minimal: null,
    low: 'high',
    medium: 'high',
    high: 'high',
  })
  assert.deepEqual(BUNDLED_MODEL_METADATA['deepseek-v4-pro'].thinkingLevelMap, {
    xhigh: null,
    max: 'max',
    minimal: null,
    low: null,
    medium: null,
    high: 'high',
  })
  assert.deepEqual(BUNDLED_MODEL_METADATA.k3.thinkingLevelMap, {
    xhigh: null,
    max: 'max',
    off: null,
    minimal: null,
    low: null,
    medium: null,
    high: null,
  })
  assert.equal(BUNDLED_MODEL_METADATA['gemini-2.0-flash'].reasoning, false)
  assert.equal(BUNDLED_MODEL_METADATA['deepseek-chat'].reasoning, false)
})

test('stored window metadata retains bundled model capabilities', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-model-metadata-merged-'))
  const path = join(directory, 'metadata.json')
  t.after(() => rm(directory, { recursive: true, force: true }))
  await writeFile(
    path,
    JSON.stringify({
      version: 1,
      models: { 'gpt-5.6-sol': { id: 'gpt-5.6-sol', contextWindow: 300_000, maxTokens: 140_000 } },
    }),
  )
  const metadata = new ModelMetadataService({ path })
  await metadata.init()

  const model = metadata.get('gpt-5.6-sol')
  assert.equal(model.contextWindow, 300_000)
  assert.equal(model.maxTokens, 140_000)
  assert.deepEqual(model.input, ['text', 'image'])
  assert.deepEqual(model.thinkingLevelMap, { off: 'none', xhigh: 'xhigh', max: 'max' })
})

test('unlisted future family models still use first-use discovery', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-model-metadata-future-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  let calls = 0
  const metadata = new ModelMetadataService({
    path: join(directory, 'metadata.json'),
    fetchImpl: async () => {
      calls += 1
      return response({
        anthropic: { models: { 'claude-future': { limit: { context: 2_000_000 } } } },
      })
    },
  })
  await metadata.init()

  assert.equal((await metadata.ensure('claude-future')).contextWindow, 2_000_000)
  assert.equal(calls, 1)
})

test('unknown model metadata is fetched once and persists across restarts', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-model-metadata-cache-'))
  const path = join(directory, 'metadata.json')
  t.after(() => rm(directory, { recursive: true, force: true }))
  let calls = 0
  const fetchImpl = async () => {
    calls += 1
    return response({
      relay: {
        models: { 'new-model': { id: 'new-model', limit: { context: 320_000, output: 64_000 } } },
      },
    })
  }
  const first = new ModelMetadataService({ path, fetchImpl })
  await first.init()

  assert.deepEqual(await first.ensure('new-model'), {
    id: 'new-model',
    contextWindow: 320_000,
    maxTokens: 64_000,
  })
  assert.equal((await first.ensure('new-model')).contextWindow, 320_000)
  assert.equal(calls, 1)

  const restored = new ModelMetadataService({ path, fetchImpl })
  await restored.init()
  assert.equal((await restored.ensure('new-model')).contextWindow, 320_000)
  assert.equal(calls, 1)
})

test('confirmed missing models are cached while network failures remain retryable', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-model-metadata-missing-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  let calls = 0
  let fail = true
  const metadata = new ModelMetadataService({
    path: join(directory, 'metadata.json'),
    fetchImpl: async () => {
      calls += 1
      if (fail) throw new Error('offline')
      return response({
        relay: { models: { known: { id: 'known', limit: { context: 100_000 } } } },
      })
    },
  })
  await metadata.init()

  assert.equal(await metadata.ensure('missing-model'), null)
  fail = false
  assert.equal(await metadata.ensure('missing-model'), null)
  assert.equal(await metadata.ensure('missing-model'), null)
  assert.equal(calls, 2)
})
