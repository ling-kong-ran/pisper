import assert from 'node:assert/strict'
import test from 'node:test'
import { browserLaunchSpec, openBrowser, shouldOpenBrowser } from '../open-browser.mjs'

test('local npm servers open a browser only when explicitly enabled', () => {
  assert.equal(shouldOpenBrowser({ host: '127.0.0.1', env: {} }), false)
  assert.equal(shouldOpenBrowser({ host: '127.0.0.1', env: { PISPER_OPEN_BROWSER: '1' } }), true)
  assert.equal(shouldOpenBrowser({ host: 'localhost', env: { PISPER_OPEN_BROWSER: '1' } }), true)
  assert.equal(shouldOpenBrowser({ host: '0.0.0.0', env: { PISPER_OPEN_BROWSER: '1' } }), false)
  assert.equal(
    shouldOpenBrowser({ host: '127.0.0.1', env: { CI: 'true', PISPER_OPEN_BROWSER: '1' } }),
    false,
  )
})

test('browser launch commands use the operating system default URL handler', () => {
  assert.deepEqual(browserLaunchSpec('http://127.0.0.1:5173', 'win32'), {
    command: 'rundll32.exe',
    args: ['url.dll,FileProtocolHandler', 'http://127.0.0.1:5173/'],
  })
  assert.deepEqual(browserLaunchSpec('https://example.com/path', 'darwin'), {
    command: 'open',
    args: ['https://example.com/path'],
  })
  assert.deepEqual(browserLaunchSpec('https://example.com/path', 'linux'), {
    command: 'xdg-open',
    args: ['https://example.com/path'],
  })
  assert.throws(() => browserLaunchSpec('file:///tmp/page.html', 'linux'), /HTTP\/HTTPS/)
})

test('browser launcher detaches without blocking the npm server', () => {
  const calls = []
  let unref = false
  const launched = openBrowser('http://127.0.0.1:5173', {
    platform: 'win32',
    spawn(command, args, options) {
      calls.push({ command, args, options })
      return {
        once() {},
        unref() {
          unref = true
        },
      }
    },
  })

  assert.equal(launched, true)
  assert.equal(unref, true)
  assert.equal(calls[0].command, 'rundll32.exe')
  assert.equal(calls[0].options.detached, true)
  assert.equal(calls[0].options.stdio, 'ignore')
})
