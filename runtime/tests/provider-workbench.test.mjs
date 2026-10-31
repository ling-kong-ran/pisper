import assert from 'node:assert/strict'
import test from 'node:test'
import { getProviderWorkbenchState } from '../../src/features/config/provider-workbench-state.ts'
import { getMoreNavigation } from '../../src/app/more-navigation.ts'
import { getSettingsNavigation } from '../../src/app/settings-navigation.ts'

const provider = (id, overrides = {}) => ({
  id,
  name: id,
  type: 'chat',
  api: 'openai-completions',
  models: [],
  configured: false,
  enabled: true,
  ...overrides,
})

test('provider workbench hides unused presets while retaining saved and disabled connections', () => {
  const config = {
    provider: 'unused-preset',
    providers: [
      provider('unused-preset'),
      provider('configured-builtin', { configured: true }),
      provider('disabled-builtin', { configured: true, enabled: false }),
      provider('incomplete-custom', { custom: true, enabled: false }),
      provider('visual-custom', { custom: true, configured: true, type: 'visual' }),
    ],
  }
  const before = structuredClone(config)
  const { providers, selected } = getProviderWorkbenchState(config, 'unused-preset')
  assert.deepEqual(
    providers.map((item) => item.id),
    ['configured-builtin', 'disabled-builtin', 'incomplete-custom', 'visual-custom'],
  )
  assert.equal(selected.id, 'configured-builtin')
  assert.deepEqual(config, before, 'filtering the view must not delete saved configuration')
})

test('provider selection survives switching and falls back after removal', () => {
  const config = {
    provider: 'first',
    defaultProvider: 'default',
    providers: [
      provider('first', { configured: true }),
      provider('default', { configured: true }),
      provider('new-connection', { custom: true }),
    ],
  }
  assert.equal(getProviderWorkbenchState(config, 'new-connection').selected.id, 'new-connection')
  assert.equal(getProviderWorkbenchState(config, 'deleted').selected.id, 'default')
  assert.equal(getProviderWorkbenchState(config, '').selected.id, 'default')
  assert.equal(
    getProviderWorkbenchState({ ...config, defaultProvider: 'deleted' }, '').selected.id,
    'first',
  )
})

test('a fresh provider configuration renders an empty workbench without selecting a preset', () => {
  for (const providers of [[], [provider('unused-preset')]]) {
    const result = getProviderWorkbenchState({ providers, provider: 'unused-preset' }, '')
    assert.deepEqual(result.providers, [])
    assert.equal(result.selected, undefined)
  }
})

test('the settings directory lists only application settings; device and extension pages live under Features', () => {
  // 规格 2.6：设置目录 = 模型 · 界面 · 通知 · 快捷键 · 更新 · 关于（界面里含语言、主题等）。
  // 远程访问 / 手机服务器 / 桌面宠物与插件、MCP、技能、星忆、渠道只在“功能”里（双归属退役），
  // 平台差异（桌面远程访问、手机服务器）随之由功能索引表达。
  const items = getSettingsNavigation((key) => key)
  assert.deepEqual(
    items.map((item) => item.section),
    ['models', 'interface', 'notifications', 'shortcuts', 'updates', 'about'],
  )
  for (const item of items) assert.equal(typeof item.label, 'string')
  for (const mobileApp of [false, true]) {
    const keys = getMoreNavigation((key) => key, { mobileApp }).flatMap((group) =>
      group.tiles.map((tile) => tile.key),
    )
    assert.equal(keys.includes('config:mobile-server'), mobileApp)
    assert.equal(keys.includes('config:remote-access'), !mobileApp)
    assert.ok(keys.includes('config:desktop-pet'))
  }
})
