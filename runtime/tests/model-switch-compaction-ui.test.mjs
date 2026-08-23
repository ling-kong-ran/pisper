import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('model switching offers context compaction only for non-empty sessions', async () => {
  const source = await readFile(
    new URL('../../src/features/chat/use-session-commands.ts', import.meta.url),
    'utf8',
  )
  assert.match(source, /shouldOfferCompaction =[\s\S]*Boolean\(current\?\.messages\?\.length\)/)
  assert.match(source, /compactAfterModelSwitchDescription/)
  assert.match(source, /confirmLabel: t\('chat:chatPage\.compactNow'\),\s+tone: 'primary'/)
  assert.match(source, /if \(confirmed\) await compactSession\(sessionId\)/)
})

test('destructive buttons keep readable contrast in Android WebView', async () => {
  const source = await readFile(
    new URL('../../src/components/ui/button.tsx', import.meta.url),
    'utf8',
  )
  assert.match(source, /bg-danger-soft text-danger hover:bg-danger-border/)
  assert.doesNotMatch(source, /bg-destructive\/10/)
})
