import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('Provider API key saving reads the live password input instead of relying only on React draft state', async () => {
  const source = await readFile('src/features/config/ConfigPage.tsx', 'utf8')
  assert.match(source, /const apiKeyInputRef = useRef<HTMLInputElement>\(null\)/)
  assert.match(source, /apiKeyInputRef\.current\?\.value \|\| draft\.apiKey/)
  assert.match(source, /autoComplete="new-password"/)
  assert.match(source, /onInput=\{\(event\) =>/)
  assert.match(source, /apiKey\.trim\(\) && !saved\.apiKeyUpdated/)
})
