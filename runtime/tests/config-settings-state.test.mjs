import assert from 'node:assert/strict'
import test from 'node:test'
import {
  configSettingsReducer,
  createConfigDraft,
  draftForProvider,
  initialConfigSettingsState,
  refreshConfigDraft,
} from '../../src/features/config/config-state.ts'

function provider(overrides = {}) {
  return {
    id: 'openai',
    name: 'OpenAI',
    type: 'chat',
    api: 'openai-responses',
    models: [
      { id: 'gpt-image', name: 'Image', kind: 'image' },
      { id: 'gpt-default', name: 'Default', kind: 'chat' },
      { id: 'gpt-preferred', name: 'Preferred', kind: 'chat', baseUrlOverride: 'model-url' },
    ],
    defaultModel: 'gpt-default',
    baseUrl: 'provider-url',
    organization: 'org',
    enabled: true,
    configured: true,
    ...overrides,
  }
}

function config(overrides = {}) {
  return {
    providers: [provider()],
    provider: 'openai',
    model: 'gpt-preferred',
    thinkingLevel: 'high',
    toolMode: 'workspace',
    ...overrides,
  }
}

test('config draft selects a preferred chat model and never defaults to a visual model', () => {
  const data = config()
  const draft = createConfigDraft(data, data.providers[0], 'gpt-preferred')
  assert.equal(draft.model, 'gpt-preferred')
  assert.equal(draft.modelBaseUrl, 'model-url')
  assert.equal(draft.baseUrl, 'provider-url')
  assert.equal(draft.thinkingLevel, 'high')
})

test('catalog refresh preserves unsaved credentials and policy while reconciling removed models', () => {
  const current = {
    ...createConfigDraft(config(), provider(), 'gpt-preferred'),
    apiKey: 'unsaved-secret',
    baseUrl: 'unsaved-provider-url',
    organization: 'unsaved-org',
    thinkingLevel: 'xhigh',
    toolMode: 'read-only',
  }
  const refreshedProvider = provider({
    models: [{ id: 'gpt-new', name: 'New', kind: 'chat', baseUrlOverride: 'new-model-url' }],
  })
  const refreshed = config({ providers: [refreshedProvider], model: 'gpt-new' })
  const next = refreshConfigDraft(refreshed, current)

  assert.equal(next.model, 'gpt-new')
  assert.equal(next.modelBaseUrl, 'new-model-url')
  assert.equal(next.apiKey, 'unsaved-secret')
  assert.equal(next.baseUrl, 'unsaved-provider-url')
  assert.equal(next.organization, 'unsaved-org')
  assert.equal(next.thinkingLevel, 'xhigh')
  assert.equal(next.toolMode, 'read-only')
})

test('provider selection preserves runtime policy and marks subsequent edits dirty', () => {
  const data = config()
  const initialDraft = { ...createConfigDraft(data, data.providers[0]), thinkingLevel: 'low' }
  const selected = draftForProvider(data, data.providers[0], initialDraft, 'gpt-preferred')
  assert.equal(selected.model, 'gpt-preferred')
  assert.equal(selected.thinkingLevel, 'low')

  let state = configSettingsReducer(initialConfigSettingsState, {
    type: 'replace',
    config: data,
    draft: selected,
    dirty: false,
  })
  state = configSettingsReducer(state, {
    type: 'patch-draft',
    patch: { apiKey: 'secret', toolMode: 'full' },
  })
  assert.equal(state.dirty, true)
  assert.equal(state.draft.apiKey, 'secret')

  state = configSettingsReducer(state, { type: 'save-succeeded', config: data })
  assert.equal(state.dirty, false)
  assert.equal(state.draft.apiKey, '')
  assert.equal(state.draft.thinkingLevel, 'low')
  assert.equal(state.draft.toolMode, 'full')
})
