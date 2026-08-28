import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('Provider settings save separately from changing the default model', async () => {
  const [modelsSettingsSource, providerDetailSource, settingsHookSource] = await Promise.all([
    readFile('src/features/config/ModelsSettings.tsx', 'utf8'),
    readFile('src/features/config/ProviderDetail.tsx', 'utf8'),
    readFile('src/features/config/useConfigSettings.ts', 'utf8'),
  ])

  assert.match(providerDetailSource, /onClick=\{\(\) => onSave\(false\)\}/)
  assert.match(providerDetailSource, /onClick=\{\(\) => onSave\(true\)\}/)
  assert.match(providerDetailSource, /configPage\.saveProviderSettings/)
  assert.match(providerDetailSource, /configPage\.setAsDefaultProvider/)
  assert.doesNotMatch(providerDetailSource, /saving \|\| !provider\.enabled/)
  assert.match(providerDetailSource, /disabled=\{saving \|\| !dirty/)
  assert.match(modelsSettingsSource, /<ProviderDetail[\s\S]*?<RuntimePolicySettings/)
  assert.doesNotMatch(modelsSettingsSource, /detailTab|config-tabs/)
  assert.match(settingsHookSource, /async \(setAsDefault = false\)/)
  assert.match(settingsHookSource, /if \(!draft \|\| !state\.dirty\) return/)
  assert.match(settingsHookSource, /configPage\.completeProviderAuthenticationBeforeSaving/)
  assert.match(settingsHookSource, /!savedProvider\?\.configured \|\| !savedProvider\.enabled/)
  assert.match(
    settingsHookSource,
    /JSON\.stringify\(\{ \.\.\.draft, apiKey, setAsDefault, enabled: true \}\)/,
  )
})

test('Provider API key saving reads the live password input instead of relying only on React draft state', async () => {
  const [providerDetailSource, settingsHookSource] = await Promise.all([
    readFile('src/features/config/ProviderDetail.tsx', 'utf8'),
    readFile('src/features/config/useConfigSettings.ts', 'utf8'),
  ])
  assert.match(settingsHookSource, /const apiKeyInputRef = useRef<HTMLInputElement>\(null\)/)
  assert.match(settingsHookSource, /apiKeyInputRef\.current\?\.value \|\| draft\.apiKey/)
  assert.match(providerDetailSource, /type="password"/)
  assert.match(providerDetailSource, /autoComplete="new-password"/)
  assert.match(providerDetailSource, /onInput=\{\(event\) =>/)
  assert.match(settingsHookSource, /apiKey\.trim\(\) && !saved\.apiKeyUpdated/)
})
