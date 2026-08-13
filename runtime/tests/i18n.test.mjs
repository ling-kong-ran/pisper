import assert from 'node:assert/strict'
import test from 'node:test'
import { i18n, translateText } from '../../src/app/i18n.ts'

test('English interface translations resolve static and interpolated messages', () => {
  assert.equal(translateText('navigation:navigation.settings', 'en-US'), 'Settings')
  assert.equal(translateText('navigation:navigation.workbench', 'en-US'), 'Workbench')
  assert.equal(translateText('navigation:navigation.chat', 'en-US'), 'Chats')
  assert.equal(translateText('plugins:toolLabels.read', 'en-US'), 'Read files')
  assert.equal(
    translateText('config:languageSettings.displayLanguage', 'en-US'),
    'Display language',
  )
  assert.equal(translateText('config:configPage.countModels', 'en-US', { count: 3 }), '3 models')
  assert.equal(
    translateText('common:app.importableProvidersMessage', 'en-US', { count: 2 }),
    'Found 2 importable provider(s) in local Codex/Claude configs. Open settings to review?',
  )
  assert.equal(
    translateText('chat:agentRunActivity.contextCompactedBeforeAfterTokens', 'en-US', {
      before: '92K',
      after: '18.5K',
    }),
    'Context compacted: 92K → 18.5K tokens',
  )
})

test('Chinese remains the default interface language', () => {
  assert.equal(translateText('config:languageSettings.displayLanguage'), '界面语言')
  assert.equal(translateText('config:configPage.countModels', 'zh-CN', { count: 3 }), '3 个模型')
  assert.equal(
    translateText('common:app.importableProvidersMessage', 'zh-CN', { count: 2 }),
    '从本地 Codex/Claude 配置中检测到 2 个可导入的提供商，是否前往设置页查看？',
  )
})

test('i18next owns the active language and resolves namespaced interpolation', async () => {
  await i18n.changeLanguage('en-US')
  assert.equal(i18n.resolvedLanguage, 'en-US')
  assert.equal(
    translateText('navigation:pageHeader.searchPage', i18n.resolvedLanguage, {
      page: 'Memory',
    }),
    'Search Memory',
  )
  await i18n.changeLanguage('zh-CN')
})
