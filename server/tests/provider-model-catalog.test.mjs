import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { ProviderModelCatalogService, inferredContextWindow } from '../services/provider-model-catalog-service.mjs'
import { BUNDLED_MODEL_METADATA } from '../services/model-metadata-service.mjs'

test('gpt-5.6 relay models use their 272k context window', () => {
  assert.equal(inferredContextWindow('gpt-5.6-sol'), 272_000)
  assert.equal(inferredContextWindow('gpt-5.6-terra'), 272_000)
  assert.equal(inferredContextWindow('gpt-5.6-luna'), 272_000)
  assert.equal(inferredContextWindow('unknown-model'), 200_000)
})

test('dynamic models do not inherit another model context window', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-catalog-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const catalog = new ProviderModelCatalogService({ path: join(directory, 'catalog.json') })
  await catalog.init()
  await catalog.sync('relay', {
    baseUrl: 'https://relay.example.test/v1',
    api: 'openai-responses',
    models: [
      { id: 'gpt-5.6-terra', name: 'GPT-5.6 Terra', kind: 'chat' },
      { id: 'unknown-model', name: 'Unknown Model', kind: 'chat' },
    ],
  })
  const template = { provider: 'relay', id: 'old-model', contextWindow: 128_000, maxTokens: 128_000 }
  const runtime = {
    getModels: (provider) => provider === 'relay' ? [template] : [],
    getModel: () => undefined,
    getAvailable: async () => [template],
    getAvailableSnapshot: () => [template],
  }

  catalog.decorateRuntime(runtime, { relay: 'https://relay.example.test/v1' })

  assert.equal(runtime.getModel('relay', 'gpt-5.6-terra').contextWindow, 272_000)
  assert.equal(runtime.getModel('relay', 'unknown-model').contextWindow, 200_000)
})

test('known visual models recover image input while explicit input remains authoritative', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-input-capability-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const metadata = { get: (id) => BUNDLED_MODEL_METADATA[id] || null }

  const decorate = async (configuredInputs = {}) => {
    const catalog = new ProviderModelCatalogService({ path: join(directory, `${Object.keys(configuredInputs).length}.json`), metadata })
    await catalog.init()
    const raw = { provider: 'relay', id: 'gpt-5.6-sol', input: ['text'], contextWindow: 128_000, maxTokens: 128_000 }
    const runtime = {
      getModels: (provider) => provider === 'relay' ? [raw] : [],
      getModel: () => raw,
      getAvailable: async () => [raw],
      getAvailableSnapshot: () => [raw],
    }
    catalog.decorateRuntime(runtime, { relay: 'https://relay.example.test/v1' }, {}, {}, configuredInputs)
    return runtime.getModel('relay', 'gpt-5.6-sol')
  }

  assert.deepEqual((await decorate()).input, ['text', 'image'])
  assert.deepEqual((await decorate({ 'relay:gpt-5.6-sol': ['text'] })).input, ['text'])
})

test('raw runtime models use metadata while explicit context configuration wins', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-raw-model-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const catalog = new ProviderModelCatalogService({ path: join(directory, 'catalog.json') })
  await catalog.init()
  const raw = { provider: 'relay', id: 'gpt-5.6-sol', contextWindow: 128_000, maxTokens: 128_000 }
  const runtime = {
    getModels: (provider) => provider === 'relay' ? [raw] : [],
    getModel: () => raw,
    getAvailable: async () => [raw],
    getAvailableSnapshot: () => [raw],
  }

  catalog.decorateRuntime(runtime, { relay: 'https://relay.example.test/v1' }, {}, { 'relay:gpt-5.6-sol': 300_000 })

  assert.equal(runtime.getModel('relay', 'gpt-5.6-sol').contextWindow, 300_000)
})
