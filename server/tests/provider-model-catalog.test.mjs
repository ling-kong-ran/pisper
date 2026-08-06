import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  ProviderModelCatalogService,
  inferredContextWindow,
} from '../services/provider-model-catalog-service.mjs'
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
  const template = {
    provider: 'relay',
    id: 'old-model',
    contextWindow: 128_000,
    maxTokens: 128_000,
  }
  const runtime = {
    getModels: (provider) => (provider === 'relay' ? [template] : []),
    getModel: () => undefined,
    getAvailable: async () => [template],
    getAvailableSnapshot: () => [template],
  }

  catalog.decorateRuntime(runtime, { relay: 'https://relay.example.test/v1' })

  assert.equal(runtime.getModel('relay', 'gpt-5.6-terra').contextWindow, 272_000)
  assert.equal(runtime.getModel('relay', 'unknown-model').contextWindow, 200_000)
  assert.deepEqual(runtime.getModel('relay', 'unknown-model').thinkingLevelMap, {
    xhigh: null,
    max: null,
  })
})

test('known visual models recover image input while explicit input remains authoritative', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-input-capability-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const metadata = { get: (id) => BUNDLED_MODEL_METADATA[id] || null }

  const decorate = async (configuredInputs = {}) => {
    const catalog = new ProviderModelCatalogService({
      path: join(directory, `${Object.keys(configuredInputs).length}.json`),
      metadata,
    })
    await catalog.init()
    const raw = {
      provider: 'relay',
      id: 'gpt-5.6-sol',
      input: ['text'],
      contextWindow: 128_000,
      maxTokens: 128_000,
    }
    const runtime = {
      getModels: (provider) => (provider === 'relay' ? [raw] : []),
      getModel: () => raw,
      getAvailable: async () => [raw],
      getAvailableSnapshot: () => [raw],
    }
    catalog.decorateRuntime(
      runtime,
      { relay: 'https://relay.example.test/v1' },
      {},
      {},
      configuredInputs,
    )
    return runtime.getModel('relay', 'gpt-5.6-sol')
  }

  const inferred = await decorate()
  assert.deepEqual(inferred.input, ['text', 'image'])
  assert.deepEqual(inferred.thinkingLevelMap, { off: 'none', xhigh: 'xhigh', max: 'max' })
  assert.deepEqual((await decorate({ 'relay:gpt-5.6-sol': ['text'] })).input, ['text'])
})

test('custom models with omitted reasoning inherit exact model metadata', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-omitted-reasoning-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const metadata = { get: (id) => BUNDLED_MODEL_METADATA[id] || null }
  const catalog = new ProviderModelCatalogService({
    path: join(directory, 'catalog.json'),
    metadata,
  })
  await catalog.init()
  const raw = {
    provider: 'codex-custom',
    id: 'gpt-5.6-sol',
    reasoning: false,
    contextWindow: 128_000,
    maxTokens: 128_000,
  }
  const runtime = {
    getModels: (provider) => (provider === 'codex-custom' ? [raw] : []),
    getModel: () => raw,
    getAvailable: async () => [raw],
    getAvailableSnapshot: () => [raw],
  }

  catalog.decorateRuntime(
    runtime,
    { 'codex-custom': 'https://relay.example.test/v1' },
    {},
    {},
    {},
    { 'codex-custom:gpt-5.6-sol': null },
  )

  const inherited = runtime.getModel('codex-custom', 'gpt-5.6-sol')
  assert.equal(inherited.reasoning, true)
  assert.deepEqual(inherited.thinkingLevelMap, { off: 'none', xhigh: 'xhigh', max: 'max' })
})

test('model thinking-level overrides remain authoritative over metadata templates', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-thinking-capability-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const metadata = { get: (id) => BUNDLED_MODEL_METADATA[id] || null }
  const catalog = new ProviderModelCatalogService({
    path: join(directory, 'catalog.json'),
    metadata,
  })
  await catalog.init()
  const raw = {
    provider: 'relay',
    id: 'gpt-5.6-sol',
    reasoning: true,
    thinkingLevelMap: { xhigh: null, max: null },
    contextWindow: 128_000,
    maxTokens: 128_000,
  }
  const runtime = {
    getModels: (provider) => (provider === 'relay' ? [raw] : []),
    getModel: () => raw,
    getAvailable: async () => [raw],
    getAvailableSnapshot: () => [raw],
  }

  catalog.decorateRuntime(runtime, { relay: 'https://relay.example.test/v1' })

  assert.deepEqual(runtime.getModel('relay', 'gpt-5.6-sol').thinkingLevelMap, {
    off: 'none',
    xhigh: null,
    max: null,
  })
})

test('dynamic relay models inherit their own thinking capabilities', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-model-capabilities-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const metadata = { get: (id) => BUNDLED_MODEL_METADATA[id] || null }
  const catalog = new ProviderModelCatalogService({
    path: join(directory, 'catalog.json'),
    metadata,
  })
  await catalog.init()
  await catalog.sync('relay', {
    baseUrl: 'https://relay.example.test/v1',
    api: 'openai-completions',
    models: [
      { id: 'glm-5.2', name: 'GLM-5.2', kind: 'chat' },
      { id: 'claude-sonnet-4-6', name: 'Claude Sonnet 4.6', kind: 'chat' },
      { id: 'gemini-3.1-pro-preview', name: 'Gemini 3.1 Pro', kind: 'chat' },
      { id: 'gemini-2.0-flash', name: 'Gemini 2.0 Flash', kind: 'chat' },
    ],
  })
  const template = {
    provider: 'relay',
    id: 'old-model',
    reasoning: true,
    contextWindow: 128_000,
    maxTokens: 128_000,
  }
  const runtime = {
    getModels: (provider) => (provider === 'relay' ? [template] : []),
    getModel: () => undefined,
    getAvailable: async () => [template],
    getAvailableSnapshot: () => [template],
  }

  catalog.decorateRuntime(runtime, { relay: 'https://relay.example.test/v1' })

  assert.deepEqual(
    runtime.getModel('relay', 'glm-5.2').thinkingLevelMap,
    BUNDLED_MODEL_METADATA['glm-5.2'].thinkingLevelMap,
  )
  assert.deepEqual(
    runtime.getModel('relay', 'claude-sonnet-4-6').thinkingLevelMap,
    BUNDLED_MODEL_METADATA['claude-sonnet-4-6'].thinkingLevelMap,
  )
  assert.deepEqual(
    runtime.getModel('relay', 'gemini-3.1-pro-preview').thinkingLevelMap,
    BUNDLED_MODEL_METADATA['gemini-3.1-pro-preview'].thinkingLevelMap,
  )
  assert.equal(runtime.getModel('relay', 'gemini-2.0-flash').reasoning, false)
})

test('custom providers inherit exact-ID capabilities from every registered SDK model', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-sdk-capabilities-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const catalog = new ProviderModelCatalogService({ path: join(directory, 'catalog.json') })
  await catalog.init()
  await catalog.sync('relay', {
    baseUrl: 'https://relay.example.test/v1',
    api: 'openai-completions',
    models: [{ id: 'sdk-known-model', name: 'SDK Known Model', kind: 'chat' }],
  })
  const official = {
    provider: 'official',
    id: 'sdk-known-model',
    reasoning: true,
    thinkingLevelMap: { minimal: null, max: 'max' },
    contextWindow: 320_000,
    maxTokens: 64_000,
  }
  const relayTemplate = {
    provider: 'relay',
    id: 'old-model',
    reasoning: true,
    contextWindow: 128_000,
    maxTokens: 32_000,
  }
  const runtime = {
    getModels: (provider) => {
      if (provider === 'official') return [official]
      if (provider === 'relay') return [relayTemplate]
      return [official, relayTemplate]
    },
    getModel: () => undefined,
    getAvailable: async () => [official, relayTemplate],
    getAvailableSnapshot: () => [official, relayTemplate],
  }

  catalog.decorateRuntime(runtime, {
    official: 'https://official.example.test/v1',
    relay: 'https://relay.example.test/v1',
  })

  assert.deepEqual(runtime.getModel('relay', 'sdk-known-model').thinkingLevelMap, {
    xhigh: null,
    max: 'max',
    minimal: null,
  })
})

test('raw runtime models use metadata while explicit context configuration wins', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-raw-model-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const catalog = new ProviderModelCatalogService({ path: join(directory, 'catalog.json') })
  await catalog.init()
  const raw = { provider: 'relay', id: 'gpt-5.6-sol', contextWindow: 128_000, maxTokens: 128_000 }
  const runtime = {
    getModels: (provider) => (provider === 'relay' ? [raw] : []),
    getModel: () => raw,
    getAvailable: async () => [raw],
    getAvailableSnapshot: () => [raw],
  }

  catalog.decorateRuntime(
    runtime,
    { relay: 'https://relay.example.test/v1' },
    {},
    { 'relay:gpt-5.6-sol': 300_000 },
  )

  assert.equal(runtime.getModel('relay', 'gpt-5.6-sol').contextWindow, 300_000)
})
