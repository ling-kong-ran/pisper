import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
test('native back delegates only to the trusted app and retains the platform fallback', async () => {
  const source = await readFile('src-tauri/mobile/android/MainActivity.kt', 'utf8')
  const handler = source.slice(
    source.indexOf('override fun handleOnBackPressed()'),
    source.indexOf('// Android 15+'),
  )
  assert.match(handler, /!isTrustedProxyOrigin\(origin\)/)
  assert.ok(
    handler.indexOf('!isTrustedProxyOrigin(origin)') < handler.indexOf('evaluateJavascript'),
  )
  assert.match(handler, /__pisperRequestBack/)
  assert.match(handler, /consumed != "true"\) fallback\(\)/)
  assert.match(handler, /onBackPressedDispatcher\.onBackPressed\(\)/)
  assert.match(handler, /isEnabled = false[\s\S]*finally \{ isEnabled = true \}/)
})
