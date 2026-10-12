import assert from 'node:assert/strict'
import test from 'node:test'
import {
  parsePosixProcesses,
  browserDescendants,
  stillOwnedProcesses,
  browserProcessTree,
  newBrowserProcessTree,
  mergeOwnedProcesses,
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

test('a later renderer in one browser never becomes part of another primary session', () => {
  const backend = { pid: 10, parent: 1, createdUtc: 'backend', path: 'pisper-server' }
  const first = { pid: 11, parent: 10, createdUtc: 'first', path: 'chrome' }
  const earlierRenderer = { pid: 12, parent: 11, createdUtc: 'earlier', path: 'chrome' }
  const laterRenderer = { pid: 13, parent: 11, createdUtc: 'later', path: 'chrome' }
  const second = { pid: 20, parent: 10, createdUtc: 'second', path: 'chrome' }
  const secondRenderer = { pid: 21, parent: 20, createdUtc: 'second-renderer', path: 'chrome' }
  const before = [backend, first, earlierRenderer]
  const current = [...before, laterRenderer, second, secondRenderer]
  const observed = newBrowserProcessTree(before, current, backend.pid)
  assert.deepEqual(observed, { root: second, rows: [second, secondRenderer] })
  assert.deepEqual(browserProcessTree(current, first), [first, earlierRenderer, laterRenderer])
  assert.deepEqual(stillOwnedProcesses([first, laterRenderer], observed.rows), [])
  assert.deepEqual(
    stillOwnedProcesses([{ ...secondRenderer, parent: 1, path: '[chrome]' }], observed.rows),
    [{ ...secondRenderer, parent: 1, path: '[chrome]' }],
    'a reparented or zombie process from the closed browser remains a real leak',
  )
})

test('browser ownership refresh retains old descendants and separates reopened process identities', () => {
  const first = { pid: 11, parent: 10, createdUtc: 'first', path: 'chrome' }
  const earlier = { pid: 12, parent: 11, createdUtc: 'earlier', path: 'chrome' }
  const later = { pid: 13, parent: 11, createdUtc: 'later', path: 'chrome' }
  const owned = mergeOwnedProcesses([first, earlier], browserProcessTree([first, later], first))
  assert.deepEqual(owned, [first, earlier, later])
  assert.deepEqual(stillOwnedProcesses([earlier], owned), [earlier])
  const reused = { ...first, createdUtc: 'reopened' }
  assert.deepEqual(browserProcessTree([reused, later], first), [])
  assert.deepEqual(stillOwnedProcesses([reused], owned), [])
  assert.deepEqual(newBrowserProcessTree([first], [reused], 10), { root: reused, rows: [reused] })
  assert.throws(() => newBrowserProcessTree([first], [first, later], 10), /found 0/)
  assert.throws(() => newBrowserProcessTree([], [first, { ...reused, pid: 20 }], 10), /found 2/)
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
