import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('Provider settings save separately from changing the default model', async () => {
  const [modelsSettingsSource, runtimeSettingsSource, settingsHookSource] = await Promise.all([
    readFile('src/features/config/ModelsSettings.tsx', 'utf8'),
    readFile('src/features/config/RuntimeSettings.tsx', 'utf8'),
    readFile('src/features/config/useConfigSettings.ts', 'utf8'),
  ])

  assert.match(runtimeSettingsSource, /onClick=\{\(\) => onSave\(false\)\}/)
  assert.match(runtimeSettingsSource, /onClick=\{\(\) => onSave\(true\)\}/)
  assert.match(runtimeSettingsSource, /configPage\.saveProviderSettings/)
  assert.match(runtimeSettingsSource, /configPage\.setAsDefaultProvider/)
  assert.doesNotMatch(runtimeSettingsSource, /saving \|\| !provider\.enabled/)
  assert.match(runtimeSettingsSource, /disabled=\{saving \|\| !dirty/)
  assert.match(
    modelsSettingsSource,
    /<ProviderSettingsActions[\s\S]*?<CredentialSettings[\s\S]*?<ProviderModelCatalog[\s\S]*?<RuntimePolicySettings/,
  )
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
  const [credentialSource, settingsHookSource] = await Promise.all([
    readFile('src/features/config/CredentialSettings.tsx', 'utf8'),
    readFile('src/features/config/useConfigSettings.ts', 'utf8'),
  ])
  assert.match(settingsHookSource, /const apiKeyInputRef = useRef<HTMLInputElement>\(null\)/)
  assert.match(settingsHookSource, /apiKeyInputRef\.current\?\.value \|\| draft\.apiKey/)
  assert.match(credentialSource, /type="password"/)
  assert.match(credentialSource, /autoComplete="new-password"/)
  assert.match(credentialSource, /onInput=\{\(event\) =>/)
  assert.match(settingsHookSource, /apiKey\.trim\(\) && !saved\.apiKeyUpdated/)
})
