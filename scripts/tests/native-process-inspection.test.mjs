import assert from 'node:assert/strict'
import test from 'node:test'
import {
  parsePosixProcesses,
  browserDescendants,
  stillOwnedProcesses,
} from '../native-process-inspection.mjs'

test('POSIX process inspection preserves app paths with spaces and only traverses owned descendants', () => {
  const rows = parsePosixProcesses(`
  10 1 Sun Oct 11 02:00:00 2026 /fixture/pisper-server
  11 10 Sun Oct 11 02:00:01 2026 /Applications/Google Chrome Dev.app/Contents/MacOS/Google Chrome Dev
  12 11 Sun Oct 11 02:00:02 2026 /Applications/Google Chrome Dev.app/Contents/Frameworks/Google Chrome Dev Helper
  13 1 Sun Oct 11 02:00:03 2026 /Applications/Google Chrome Dev.app/Contents/MacOS/Google Chrome Dev
  14 10 Sun Oct 11 02:00:04 2026 /usr/bin/sh
  15 14 Sun Oct 11 02:00:05 2026 /usr/bin/chromium
`)
  const owned = browserDescendants(rows, 10)
  assert.deepEqual(
    owned.map((row) => row.pid),
    [11, 12, 15],
  )
  assert.ok(owned[0].path.endsWith('Google Chrome Dev'))
  assert.deepEqual(stillOwnedProcesses([{ ...owned[0], createdUtc: 'later' }], owned), [])
  assert.equal(
    stillOwnedProcesses([{ ...owned[0], path: '/unrelated' }], owned).length,
    1,
    'exec or zombie command changes do not count as reaping',
  )
  assert.deepEqual(stillOwnedProcesses([owned[0]], owned), [owned[0]])
})

test('browser cleanup retains plugins installed by later restart fixtures', async () => {
  const { restoreBrowserTools } = await import('../smoke-rust-browser.mjs')
  const current = ['read', 'browser_automation', 'later-plugin-tool', 'plugin_create']
  assert.deepEqual(restoreBrowserTools(current, false), [
    'read',
    'later-plugin-tool',
    'plugin_create',
  ])
  assert.deepEqual(new Set(restoreBrowserTools(current, true)), new Set(current))
  assert.deepEqual(restoreBrowserTools(['read'], true), ['read', 'browser_automation'])
})
