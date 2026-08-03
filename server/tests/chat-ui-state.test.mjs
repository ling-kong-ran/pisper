import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { createPrimaryActionRegistry } from '../../src/app/primary-action.ts'
import {
  mergeSessionLists,
  recentSessionCwd,
  removeTiledSession,
  toggleTiledSession,
} from '../../src/features/chat/session-list.ts'

test('primary action remains callable until its page registration is disposed', () => {
  const registry = createPrimaryActionRegistry()
  let calls = 0
  const dispose = registry.register(() => { calls += 1 })

  registry.invoke()
  registry.invoke()
  assert.equal(calls, 2)

  dispose()
  registry.invoke()
  assert.equal(calls, 2)
})

test('a queued primary action runs once when a lazy page registers', () => {
  const registry = createPrimaryActionRegistry()
  let calls = 0

  registry.invoke()
  registry.register(() => { calls += 1 })
  assert.equal(calls, 1)
})

test('disposing an old page action does not clear the newly registered action', () => {
  const registry = createPrimaryActionRegistry()
  let calls = 0
  const disposeOld = registry.register(() => {})
  registry.register(() => { calls += 1 })

  disposeOld()
  registry.invoke()
  assert.equal(calls, 1)
})

test('stale initial session lists preserve an optimistically created session', () => {
  const optimistic = { id: 'new-session', name: '新会话' }
  const stale = [{ id: 'existing-session', name: '旧会话' }]

  assert.deepEqual(mergeSessionLists([optimistic], stale), [stale[0], optimistic])
})

test('new sessions inherit the most recently listed workspace', async () => {
  const sessions = [
    { id: 'latest-without-cwd', modified: '2026-08-02T02:00:00Z', cwd: '  ' },
    { id: 'latest-workspace', modified: '2026-08-02T01:00:00Z', cwd: 'E:\\code\\latest' },
    { id: 'older-workspace', modified: '2026-08-01T01:00:00Z', cwd: 'E:\\code\\older' },
  ]
  assert.equal(recentSessionCwd(sessions), 'E:\\code\\latest')
  assert.equal(recentSessionCwd([]), '')

  const [catalog, api] = await Promise.all([
    readFile('src/features/chat/use-session-catalog.ts', 'utf8'),
    readFile('src/features/chat/chat-api.ts', 'utf8'),
  ])
  assert.match(catalog, /recentSessionCwd\(sessionsRef\.current\)/)
  assert.match(api, /data: \{ name, \.\.\.\(cwd \? \{ cwd \} : \{\}\) \}/)
})

test('removing a tiled session keeps the session itself available elsewhere', () => {
  assert.deepEqual(removeTiledSession(['first', 'second', 'third'], 'second'), ['first', 'third'])
})

test('a session can be added to and removed from the tiled set', () => {
  assert.deepEqual(toggleTiledSession(['first'], 'second'), ['first', 'second'])
  assert.deepEqual(toggleTiledSession(['first', 'second'], 'second'), ['first'])
})

test('the chat composer exposes the global command palette shortcut', async () => {
  const [app, events, focus, styles, english, chinese] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/features/chat/events.ts', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
    readFile('src/locales/en-US/chat.json', 'utf8').then(JSON.parse),
    readFile('src/locales/zh-CN/chat.json', 'utf8').then(JSON.parse),
  ])

  assert.match(events, /COMMAND_PALETTE_REQUESTED_EVENT/)
  assert.match(app, /addEventListener\(COMMAND_PALETTE_REQUESTED_EVENT, openCommandPalette\)/)
  assert.match(focus, /className="command-palette-trigger"/)
  assert.match(focus, /onClick=\{requestCommandPalette\}/)
  assert.match(focus, /<kbd>\{COMMAND_PALETTE_SHORTCUT\}<\/kbd>/)
  assert.match(styles, /\.command-palette-trigger kbd/)
  assert.equal(english['focusSession.openCommandPaletteShortcut'], 'Open command palette ({shortcut})')
  assert.equal(chinese['focusSession.openCommandPaletteShortcut'], '打开命令面板（{shortcut}）')
})

test('the git changes badge uses the theme-aware contrasting text color', async () => {
  const styles = await readFile('src/index.css', 'utf8')
  assert.match(styles, /\.git-changes-trigger > i \{[^}]*color: var\(--on-accent\)/)
})
