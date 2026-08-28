import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('quick setup wizard saves provider config and default model in one step', async () => {
  const [wizardSource, modelsSettingsSource] = await Promise.all([
    readFile('src/features/config/QuickSetupWizard.tsx', 'utf8'),
    readFile('src/features/config/ModelsSettings.tsx', 'utf8'),
  ])

  assert.match(wizardSource, /setAsDefault: true/)
  assert.match(wizardSource, /enabled: true/)
  assert.match(wizardSource, /apiKey: readApiKey\(\)/)
  // 高级设置（协议/端点/组织）折叠在向导第 2 步内
  assert.match(wizardSource, /configPage\.advancedSettings/)
  assert.match(wizardSource, /configPage\.apiProtocol/)
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
