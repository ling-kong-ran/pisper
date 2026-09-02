import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('quick setup wizard saves provider config after fetching and selecting a model', async () => {
  const [wizardSource, modelsSettingsSource] = await Promise.all([
    readFile('src/features/config/QuickSetupWizard.tsx', 'utf8'),
    readFile('src/features/config/ModelsSettings.tsx', 'utf8'),
  ])

  assert.match(wizardSource, /setAsDefault: true/)
  assert.match(wizardSource, /enabled: true/)
  assert.match(wizardSource, /apiKey: readApiKey\(\)/)
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

test('Provider API key saving reads the live password input instead of relying only on React state', async () => {
  const wizardSource = await readFile('src/features/config/QuickSetupWizard.tsx', 'utf8')
  assert.match(wizardSource, /const apiKeyInputRef = useRef<HTMLInputElement>\(null\)/)
  assert.match(wizardSource, /apiKeyInputRef\.current\?\.value/)
  assert.match(wizardSource, /type="password"/)
  assert.match(wizardSource, /autoComplete="new-password"/)
  assert.match(wizardSource, /onInput=\{\(event\) =>/)
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
  assert.doesNotMatch(connectionSource, /provider\.type === 'visual'/)
  assert.match(visualSource, /value=\{provider\.configured && provider\.enabled\}/)
  assert.match(visualSource, /onToggleProvider\(provider, enabled\)/)
  assert.match(modelsSource, /onToggleProvider=\{settings\.toggleProvider\}/)
})
