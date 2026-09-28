import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  BROWSER_PREFERENCE_KEYS,
  BrowserPreferenceError,
  parseBrowserPreferenceSnapshot,
} from '../../shared/browser-preferences.mjs'
import { BrowserPreferencesService } from '../services/browser-preferences-service.mjs'
import { handleLocalBrowserPreferences } from '../http/local-browser-preferences.mjs'

test('browser preferences survive a runtime restart and concurrent writes', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-browser-preferences-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const service = new BrowserPreferencesService({ dataDir })
  await Promise.all([
    service.update({ 'pisper-session-context-layout': '{"state":{"open":true}}' }),
    service.update({ 'pisper-ui': '{"state":{"theme":"dark"}}' }),
  ])
  await service.dispose()

  const saved = JSON.parse(await readFile(join(dataDir, 'pisper-browser-preferences.json'), 'utf8'))
  assert.equal(saved.values['pisper-session-context-layout'], '{"state":{"open":true}}')
  assert.equal(saved.values['pisper-ui'], '{"state":{"theme":"dark"}}')
  const restarted = new BrowserPreferencesService({ dataDir })
  assert.deepEqual(await restarted.snapshot(), saved)
  await restarted.update({ 'pisper-session-context-layout': null })
  assert.equal((await restarted.snapshot()).values['pisper-session-context-layout'], null)
  await restarted.dispose()
})

test('browser preferences reject unlisted keys, oversized values and malformed snapshots', () => {
  assert.equal(BROWSER_PREFERENCE_KEYS.includes('pisper-chat-layout'), false)
  assert.deepEqual(
    {
      ...parseBrowserPreferenceSnapshot({
        version: 1,
        values: { 'pisper-ui': '{}', 'pisper-language': 'zh-CN' },
        revisions: { 'pisper-ui': 100 },
      }).revisions,
    },
    { 'pisper-ui': 100 },
  )
  assert.throws(
    () => parseBrowserPreferenceSnapshot({ version: 1, values: { 'api-key': 'secret' } }),
    BrowserPreferenceError,
  )
  assert.throws(
    () =>
      parseBrowserPreferenceSnapshot({ version: 1, values: { 'pisper-ui': 'x'.repeat(524289) } }),
    BrowserPreferenceError,
  )
  assert.throws(
    () => parseBrowserPreferenceSnapshot({ version: 2, values: {} }),
    BrowserPreferenceError,
  )
})

test('retired large chat layout is discarded while other old snapshot preferences survive', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-browser-preferences-legacy-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const path = join(dataDir, 'pisper-browser-preferences.json')
  await writeFile(
    path,
    JSON.stringify({
      version: 1,
      values: {
        'pisper-chat-layout': 'x'.repeat(20 * 64 * 1024),
        'pisper-session-context-layout': '{"state":{"open":false}}',
      },
      revisions: { 'pisper-chat-layout': 50, 'pisper-session-context-layout': 100 },
    }),
  )
  const service = new BrowserPreferencesService({ dataDir })
  const restored = await service.snapshot()
  assert.equal(restored.values['pisper-session-context-layout'], '{"state":{"open":false}}')
  assert.equal(restored.revisions['pisper-session-context-layout'], 100)
  assert.equal(Object.hasOwn(restored.values, 'pisper-chat-layout'), false)
  assert.equal(Object.hasOwn(restored.revisions, 'pisper-chat-layout'), false)
  await assert.rejects(service.update({ 'pisper-chat-layout': 'new' }), BrowserPreferenceError)
  await service.update({ 'pisper-ui': 'saved' })
  const persisted = JSON.parse(await readFile(path, 'utf8'))
  assert.equal(persisted.values['pisper-ui'], 'saved')
  assert.equal(Object.hasOwn(persisted.values, 'pisper-chat-layout'), false)
  await service.dispose()
})

test('cumulative preference size is rejected before it makes the restart snapshot unreadable', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-browser-preferences-total-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const service = new BrowserPreferencesService({ dataDir })
  const large = 'x'.repeat(400 * 1024)
  const first = Object.fromEntries(
    ['pisper-ui', 'pisper-language', 'pisper-shortcuts', 'pisper-terminal-panel'].map((key) => [
      key,
      large,
    ]),
  )
  await service.update(first)
  const before = await service.snapshot()
  await assert.rejects(
    service.update({ 'pisper-workspace-order': large, 'pisper-floating-widgets': large }),
    BrowserPreferenceError,
  )
  assert.deepEqual(await service.snapshot(), before)
  await service.dispose()
  const restarted = new BrowserPreferencesService({ dataDir })
  assert.deepEqual(await restarted.snapshot(), before)
  await restarted.update({ 'pisper-ui': 'small' })
  assert.equal((await restarted.snapshot()).values['pisper-ui'], 'small')
  await restarted.dispose()
})

test('a delayed write or pagehide beacon cannot replace a newer context choice', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-browser-preferences-order-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const service = new BrowserPreferencesService({ dataDir })
  const key = 'pisper-session-context-layout'
  const open = '{"state":{"open":true}}'
  const closed = '{"state":{"open":false}}'
  await service.update({ [key]: open }, { [key]: 100 })
  await service.update({ [key]: closed }, { [key]: 200 })
  await service.update({ [key]: open }, { [key]: 100 })
  assert.equal((await service.snapshot()).values[key], closed)
  await service.dispose()
  const restarted = new BrowserPreferencesService({ dataDir })
  assert.equal((await restarted.snapshot()).values[key], closed)
  await restarted.update({ [key]: open }, { [key]: 100 })
  assert.equal((await restarted.snapshot()).values[key], closed)
  await restarted.dispose()
})

test('local preference HTTP endpoint restores the same state on a new loopback port', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-page-state-http-'))
  t.after(() => rm(dataDir, { recursive: true, force: true }))
  const service = new BrowserPreferencesService({ dataDir })
  t.after(() => service.dispose())

  async function start() {
    const server = createServer((req, res) => {
      const address = server.address()
      const origin = `http://127.0.0.1:${address.port}`
      const url = new URL(req.url, origin)
      void handleLocalBrowserPreferences(req, res, url, service, { remote: false, origin })
    })
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
    t.after(() => new Promise((resolve) => server.close(resolve)))
    return `http://127.0.0.1:${server.address().port}`
  }

  const first = await start()
  const write = await fetch(`${first}/api/local/browser-preferences`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json', Origin: first },
    body: JSON.stringify({
      updates: { 'pisper-session-context-layout': '{"state":{"open":true}}' },
    }),
  })
  assert.equal(write.status, 204)
  const rejected = await fetch(`${first}/api/local/browser-preferences`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json', Origin: 'http://other.invalid' },
    body: JSON.stringify({ updates: { 'pisper-ui': '{}' } }),
  })
  assert.equal(rejected.status, 403)
  const invalid = await fetch(`${first}/api/local/browser-preferences`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json', Origin: first },
    body: '{invalid',
  })
  assert.equal(invalid.status, 400)
  assert.equal((await invalid.json()).code, 'browser_preferences_invalid')
  const oversized = await fetch(`${first}/api/local/browser-preferences`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json', Origin: first },
    body: JSON.stringify({ updates: { 'pisper-ui': 'x'.repeat(610 * 1024) } }),
  })
  assert.equal(oversized.status, 400)
  // 解码后 1.2 MiB 在协议预算内；控制字符转义后请求体约 7.2 MiB。
  const escaped = '\u0000'.repeat(300 * 1024)
  const largeValidUpdates = Object.fromEntries(
    ['pisper-ui', 'pisper-language', 'pisper-shortcuts', 'pisper-terminal-panel'].map((key) => [
      key,
      escaped,
    ]),
  )
  const largeValidBody = JSON.stringify({ updates: largeValidUpdates })
  assert.ok(Buffer.byteLength(largeValidBody) > 600 * 1024)
  const largeValid = await fetch(`${first}/api/local/browser-preferences`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json', Origin: first },
    body: largeValidBody,
  })
  assert.equal(largeValid.status, 204)
  const tooLarge = 'x'.repeat(450 * 1024)
  const excess = await fetch(`${first}/api/local/browser-preferences`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json', Origin: first },
    body: JSON.stringify({
      updates: Object.fromEntries(
        [
          'pisper-ui',
          'pisper-language',
          'pisper-shortcuts',
          'pisper-terminal-panel',
          'pisper-workspace-order',
        ].map((key) => [key, tooLarge]),
      ),
    }),
  })
  assert.equal(excess.status, 400)
  assert.equal((await excess.json()).code, 'browser_preferences_invalid')
  const second = await start()
  assert.notEqual(first, second)
  const restored = await fetch(`${second}/api/local/browser-preferences`).then((response) =>
    response.json(),
  )
  assert.equal(restored.values['pisper-session-context-layout'], '{"state":{"open":true}}')
  assert.equal(restored.values['pisper-ui'], escaped)
})
