import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

import { normalizeTokenUsage } from '../../src/lib/format.ts'

test('status bar usage normalization tolerates incomplete mobile runtime responses', () => {
  assert.deepEqual(normalizeTokenUsage(undefined), {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    reasoning: 0,
    totalTokens: 0,
  })
  assert.deepEqual(normalizeTokenUsage({ input: 12, output: 5 }), {
    input: 12,
    output: 5,
    cacheRead: 0,
    cacheWrite: 0,
    reasoning: 0,
    totalTokens: 17,
  })
})

test('status bar usage normalization accepts numeric strings and rejects invalid values', () => {
  assert.deepEqual(
    normalizeTokenUsage({
      input: '1,000',
      output: '20',
      cacheRead: -4,
      cacheWrite: 3,
      reasoning: Number.POSITIVE_INFINITY,
      total: '30',
    }),
    {
      input: 0,
      output: 20,
      cacheRead: 0,
      cacheWrite: 3,
      reasoning: 0,
      totalTokens: 30,
    },
  )
})

test('mobile app waits for client detection and does not mount the desktop-only status bar', async () => {
  const [app, statusBar] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/components/layout/StatusBar.tsx', 'utf8'),
  ])

  assert.match(app, /const clientLoaded = useClientStore\(\(state\) => state\.loaded\)/)
  assert.match(
    app,
    /\{clientLoaded && !mobileApp && <StatusBar page=\{page\} pluginStats=\{pluginStats\} \/>\}/,
  )
  assert.match(
    statusBar,
    /setUsage\(normalizeTokenUsage\(await apiJson<unknown>\('\/api\/usage\/today'\)\)\)/,
  )
})
