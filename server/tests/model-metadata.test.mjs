import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { ModelMetadataService } from '../services/model-metadata-service.mjs'

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
    fetchImpl: async () => { calls += 1; throw new Error('unexpected request') },
  })
  await metadata.init()

  const expected = {
    'gpt-5.6-terra': [272_000, undefined],
    'claude-sonnet-4-6': [1_000_000, 128_000],
    'claude-opus-4-5': [200_000, 64_000],
    'glm-5.2': [1_000_000, 131_072],
    'glm-4.7': [204_800, 131_072],
    'deepseek-chat': [128_000, undefined],
    'deepseek-v4-pro': [1_000_000, 384_000],
    k2p7: [262_144, 32_768],
    k3: [1_048_576, 131_072],
    'gemini-2.5-pro': [1_048_576, 65_536],
    'gemini-3.1-pro-preview': [1_048_576, 65_536],
  }
  for (const [modelId, [contextWindow, maxTokens]] of Object.entries(expected)) {
    const model = await metadata.ensure(modelId)
    assert.equal(model.contextWindow, contextWindow, modelId)
    assert.equal(model.maxTokens, maxTokens, modelId)
  }
  assert.deepEqual((await metadata.ensure('gpt-5.6-sol')).input, ['text', 'image'])
  assert.equal(calls, 0)
})

test('unlisted future family models still use first-use discovery', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-model-metadata-future-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  let calls = 0
  const metadata = new ModelMetadataService({
    path: join(directory, 'metadata.json'),
    fetchImpl: async () => {
      calls += 1
      return response({ anthropic: { models: { 'claude-future': { limit: { context: 2_000_000 } } } } })
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
    return response({ relay: { models: { 'new-model': { id: 'new-model', limit: { context: 320_000, output: 64_000 } } } } })
  }
  const first = new ModelMetadataService({ path, fetchImpl })
  await first.init()

  assert.deepEqual(await first.ensure('new-model'), { id: 'new-model', contextWindow: 320_000, maxTokens: 64_000 })
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
      return response({ relay: { models: { known: { id: 'known', limit: { context: 100_000 } } } } })
    },
  })
  await metadata.init()

  assert.equal(await metadata.ensure('missing-model'), null)
  fail = false
  assert.equal(await metadata.ensure('missing-model'), null)
  assert.equal(await metadata.ensure('missing-model'), null)
  assert.equal(calls, 2)
})
