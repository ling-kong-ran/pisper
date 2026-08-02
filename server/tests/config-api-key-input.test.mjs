import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

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
