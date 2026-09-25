import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('quick setup wizard saves provider config after fetching and selecting a model', async () => {
  const [wizardSource, modelsSettingsSource] = await Promise.all([
    readFile('src/features/config/QuickSetupWizard.tsx', 'utf8'),
    readFile('src/features/config/ModelsSettings.tsx', 'utf8'),
  ])

  assert.match(wizardSource, /setAsDefault: false/)
  assert.match(wizardSource, /enabled: true/)
  assert.match(wizardSource, /apiKey,/)
  assert.match(wizardSource, /apiKeyDraft\.trim\(\)/)
  assert.match(wizardSource, /createProviderConnectionId/)
  // 向导先填写 Base URL，再选择协议，第三步获取模型列表。
  assert.match(wizardSource, /configPage\.quickSetupStepBaseUrl/)
  assert.match(wizardSource, /configPage\.quickSetupStepProtocol/)
  assert.match(wizardSource, /discover-connection/)
  assert.match(wizardSource, /configPage\.apiProtocol/)
  // 发现接口不可用时允许用目录已有模型继续（kimi-coding 等无 /models 端点的 Provider）
  assert.match(wizardSource, /configPage\.discoverFailedUsingExisting/)
  assert.match(wizardSource, /authFailure/)
  assert.match(wizardSource, /providerType === 'visual'/)
  assert.match(wizardSource, /model\.kind === 'chat'/)
  assert.match(modelsSettingsSource, /<QuickSetupWizard/)
  assert.doesNotMatch(modelsSettingsSource, /detailTab|config-tabs/)
})

test('Provider API key input uses a single password value without an add confirmation', async () => {
  const [wizardSource, dialogSource, keyListSource] = await Promise.all([
    readFile('src/features/config/QuickSetupWizard.tsx', 'utf8'),
    readFile('src/features/config/ProviderDialogs.tsx', 'utf8'),
    readFile('src/features/config/ApiKeyInput.tsx', 'utf8'),
  ])
  assert.match(wizardSource, /<ApiKeyInput/)
  assert.match(dialogSource, /<ApiKeyInput/)
  assert.match(keyListSource, /type="password"/)
  assert.match(keyListSource, /autoComplete="new-password"/)
  assert.doesNotMatch(keyListSource, /Plus|onKeyDown/)
  assert.match(dialogSource, /apiKeyDraft\.trim\(\)/)
})

test('visual Provider settings expose a direct connection editor and hide unused presets', async () => {
  const [dialogSource, modelsSource, connectionSource, visualSource] = await Promise.all([
    readFile('src/features/config/ProviderDialogs.tsx', 'utf8'),
    readFile('src/features/config/ModelsSettings.tsx', 'utf8'),
    readFile('src/features/config/ConnectionList.tsx', 'utf8'),
    readFile('src/features/config/VisualGenerationSettings.tsx', 'utf8'),
  ])
  assert.match(dialogSource, /initialProvider\?: ProviderConfig/)
  assert.match(dialogSource, /apiJson<ConfigData>\('\/api\/config'/)
  assert.match(modelsSource, /onEditVisualProvider=/)
  // 视觉供应商不混入对话连接列表，统一由视觉生成专区的「视觉连接」提供启停开关。
  assert.match(connectionSource, /provider\.configured \|\| provider\.custom\)/)
  assert.match(modelsSource, /<ProviderWorkbench/)
  assert.match(modelsSource, /setProviderModal\(\{ providerType: 'visual', provider \}\)/)
  assert.doesNotMatch(connectionSource, /provider\.type === 'visual'/)
  // 视觉连接与对话连接共用同一套卡片网格（ConnectionCardGrid），启停/删除交互一致。
  assert.match(visualSource, /<ConnectionCardGrid/)
  assert.match(visualSource, /onToggle=\{onToggleProvider\}/)
  assert.match(visualSource, /onDelete=\{onDeleteProvider\}/)
  assert.match(connectionSource, /value=\{provider\.configured && provider\.enabled\}/)
  assert.match(modelsSource, /onToggleProvider=\{settings\.toggleProvider\}/)
})
