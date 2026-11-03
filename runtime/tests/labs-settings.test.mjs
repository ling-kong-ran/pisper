import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { getSettingsNavigation } from '../../src/app/settings-navigation.ts'
import { parseBetaFeatures } from '../../src/features/labs/labs-api.ts'
import { useBetaFeaturesStore } from '../../src/features/labs/beta-features-store.ts'
import { searchConfig } from '../../src/features/config/config-search.ts'
import { LEGACY_RUNTIME_CAPABILITIES } from '../../src/types/runtime-capabilities.ts'

const root = fileURLToPath(new URL('../../', import.meta.url))

const feature = (id, overrides = {}) => ({
  id,
  stage: 'experimental',
  feedbackUrl: `https://github.com/example/pisper/issues/${id}`,
  enabled: false,
  available: true,
  unavailableReason: null,
  ...overrides,
})

function withCapabilities(features) {
  return {
    ...LEGACY_RUNTIME_CAPABILITIES,
    features: { ...LEGACY_RUNTIME_CAPABILITIES.features, ...features },
  }
}

test('the Labs section appears between Updates and About only when the Runtime declares it', () => {
  const sections = (capabilities) =>
    getSettingsNavigation((key) => key, { capabilities }).map((item) => item.section)
  assert.deepEqual(sections(withCapabilities({ betaFeatures: true })), [
    'models',
    'interface',
    'notifications',
    'shortcuts',
    'updates',
    'labs',
    'about',
  ])
  // 旧 Runtime 没有接口：分区不出现，也不会去请求。
  assert.equal(sections(LEGACY_RUNTIME_CAPABILITIES).includes('labs'), false)
  const search = (capabilities) =>
    searchConfig('beta', 'en-US', { capabilities }).map((match) => match.entry.section)
  assert.ok(search(withCapabilities({ betaFeatures: true })).includes('labs'))
  assert.equal(search(LEGACY_RUNTIME_CAPABILITIES).includes('labs'), false)
})

test('beta feature responses are validated and non-GitHub feedback links are dropped', () => {
  assert.deepEqual(
    parseBetaFeatures({ features: [feature('bots', { feedbackUrl: 'https://evil.example/x' })] }),
    [feature('bots', { feedbackUrl: '' })],
  )
  for (const bad of [
    null,
    { features: 'bots' },
    { features: [feature('Bots')] },
    { features: [feature('bots', { stage: 'alpha' })] },
    { features: [feature('bots', { enabled: 'yes' })] },
    { features: [feature('bots', { unavailableReason: 'platform' })] },
  ])
    assert.throws(() => parseBetaFeatures(bad), undefined, JSON.stringify(bad))
})

// 用可控的 fetch 驱动 store：每个请求都挂起，由测试决定何时、以什么结果返回。
function controlledFetch(t) {
  const original = globalThis.fetch
  const calls = []
  globalThis.fetch = (url, init = {}) =>
    new Promise((resolve) => {
      calls.push({
        url: String(url),
        method: init.method || 'GET',
        body: init.body ? JSON.parse(init.body) : undefined,
        reply: (status, body) =>
          resolve(
            new Response(JSON.stringify(body), {
              status,
              headers: { 'Content-Type': 'application/json' },
            }),
          ),
      })
    })
  t.after(() => {
    globalThis.fetch = original
    useBetaFeaturesStore.setState({
      features: [],
      status: 'idle',
      error: '',
      pending: {},
      rowErrors: {},
    })
  })
  return calls
}

const flush = () => new Promise((resolve) => setImmediate(resolve))

test('toggling shows the new value at once, disables the switch while saving and keeps the response', async (t) => {
  const calls = controlledFetch(t)
  const store = useBetaFeaturesStore
  const loading = store.getState().load()
  await flush()
  assert.equal(store.getState().status, 'loading')
  calls[0].reply(200, { features: [feature('bots')] })
  await loading
  assert.equal(store.getState().status, 'ready')

  const saving = store.getState().setEnabled('bots', true)
  assert.equal(store.getState().features[0].enabled, true)
  assert.equal(store.getState().pending.bots, true)
  // 保存完成前再切同一个开关不会发第二个请求。
  await store.getState().setEnabled('bots', false)
  await flush()
  assert.equal(calls.length, 2)
  assert.deepEqual(calls[1].body, { bots: true })
  assert.equal(calls[1].method, 'PATCH')
  calls[1].reply(200, { features: [feature('bots', { enabled: true })] })
  await saving
  assert.equal(store.getState().features[0].enabled, true)
  assert.deepEqual(store.getState().pending, {})
})

test('a failed save restores the switch and remembers the wanted value for retry', async (t) => {
  const calls = controlledFetch(t)
  const store = useBetaFeaturesStore
  store.setState({ features: [feature('bots')], status: 'ready' })
  const saving = store.getState().setEnabled('bots', true)
  await flush()
  calls[0].reply(400, { error: '实验功能 bots 的开关必须是布尔值。' })
  await saving
  assert.equal(store.getState().features[0].enabled, false)
  assert.deepEqual(store.getState().rowErrors.bots, {
    message: '实验功能 bots 的开关必须是布尔值。',
    desired: true,
  })
  // 重试会清掉旧错误。
  const retry = store.getState().setEnabled('bots', true)
  assert.equal(store.getState().rowErrors.bots, undefined)
  await flush()
  calls[1].reply(200, { features: [feature('bots', { enabled: true })] })
  await retry
  assert.equal(store.getState().features[0].enabled, true)
})

test('an earlier response does not flip back another switch that is still saving', async (t) => {
  const calls = controlledFetch(t)
  const store = useBetaFeaturesStore
  store.setState({ features: [feature('bots'), feature('quiet-mode')], status: 'ready' })
  const first = store.getState().setEnabled('bots', true)
  const second = store.getState().setEnabled('quiet-mode', true)
  await flush()
  // 先写入的 bots 的响应里，quiet-mode 还是旧值。
  calls[0].reply(200, { features: [feature('bots', { enabled: true }), feature('quiet-mode')] })
  await first
  assert.equal(store.getState().features[1].enabled, true)
  calls[1].reply(200, {
    features: [feature('bots', { enabled: true }), feature('quiet-mode', { enabled: true })],
  })
  await second
  assert.deepEqual(
    store.getState().features.map((item) => item.enabled),
    [true, true],
  )
})

test('unavailable features cannot be toggled', async (t) => {
  const calls = controlledFetch(t)
  const store = useBetaFeaturesStore
  store.setState({
    features: [feature('bots', { available: false, unavailableReason: 'runtime' })],
    status: 'ready',
  })
  await store.getState().setEnabled('bots', true)
  assert.equal(calls.length, 0)
  assert.equal(store.getState().features[0].enabled, false)
})

test('the Labs page uses the shared controls and reaches the Runtime only through its API module', async () => {
  const page = await readFile(join(root, 'src/features/labs/LabsSettings.tsx'), 'utf8')
  assert.match(page, /from '@\/components\/ui\/switch'/)
  assert.match(page, /<InlineEmptyState\s+compact/)
  assert.match(page, /<FeatureAlert/)
  assert.doesNotMatch(page, /\bfetch\(|<input\b|<select\b/)
  const configPage = await readFile(join(root, 'src/features/config/ConfigPage.tsx'), 'utf8')
  assert.match(configPage, /from '@\/features\/labs\/public'/)
})

test('a direct link to a section the legacy contract hides waits for capabilities before redirecting', async () => {
  // 旧 Runtime 合同里实验功能不可用：能力加载前就纠正路由，直达 /config/labs 会被改成模型。
  const app = await readFile(join(root, 'src/App.tsx'), 'utf8')
  assert.match(
    app,
    /if \(capabilitiesLoaded && page === 'config' && requestedConfigSection !== configSection\)/,
  )
})
