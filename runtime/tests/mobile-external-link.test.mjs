import assert from 'node:assert/strict'
import test from 'node:test'
import { readFile } from 'node:fs/promises'

import { openMobileExternalLink } from '../../src/lib/mobile-external-link.ts'
import { shouldOpenWebPreview } from '../../src/lib/web-preview.ts'

test('mobile external browser command is registered and permitted', async () => {
  const [registration, permissions] = await Promise.all([
    readFile(new URL('../../src-tauri/src/mobile/mod.rs', import.meta.url), 'utf8'),
    readFile(new URL('../../src-tauri/permissions/mobile.toml', import.meta.url), 'utf8'),
  ])
  assert.match(registration, /external_links::mobile_open_external_url/)
  assert.match(permissions, /"mobile_open_external_url"/)
})

function withWindow(value, run) {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', { configurable: true, value })
  try {
    return run()
  } finally {
    if (previous) Object.defineProperty(globalThis, 'window', previous)
    else delete globalThis.window
  }
}

test('mobile external links open through the native browser instead of navigating the WebView', async () => {
  const calls = []
  let prevented = false
  const opened = withWindow(
    {
      __PISPER_MOBILE_APP__: true,
      __TAURI__: {
        core: {
          invoke: async (...args) => {
            calls.push(args)
            return true
          },
        },
      },
    },
    () =>
      openMobileExternalLink(
        { preventDefault: () => (prevented = true) },
        'https://github.com/ling-kong-ran/pisper',
      ),
  )
  assert.equal(prevented, true)
  assert.equal(await opened, true)
  assert.deepEqual(calls, [
    ['mobile_open_external_url', { url: 'https://github.com/ling-kong-ran/pisper' }],
  ])
})

test('browser links keep native anchor navigation', () => {
  let prevented = false
  const opened = withWindow({ __PISPER_MOBILE_APP__: false }, () =>
    openMobileExternalLink(
      { preventDefault: () => (prevented = true) },
      'https://github.com/ling-kong-ran/pisper',
    ),
  )
  assert.equal(opened, null)
  assert.equal(prevented, false)
})

test('mobile links report a missing native bridge without navigating the WebView', async () => {
  let prevented = false
  const opened = withWindow({ __PISPER_MOBILE_APP__: true }, () =>
    openMobileExternalLink(
      { preventDefault: () => (prevented = true) },
      'https://github.com/ling-kong-ran/pisper',
    ),
  )
  assert.equal(prevented, true)
  await assert.rejects(opened, /原生桥不可用/)
})

test('settings external links bypass the capture preview before native click handlers run', async () => {
  const [about, updates] = await Promise.all([
    readFile(new URL('../../src/features/config/AboutSettings.tsx', import.meta.url), 'utf8'),
    readFile(new URL('../../src/features/config/UpdateSettings.tsx', import.meta.url), 'utf8'),
  ])
  const aboutAnchors = [...about.matchAll(/<a\b[^>]*>/g)]
  assert.equal(aboutAnchors.length, 2)
  for (const [anchor] of aboutAnchors) {
    assert.match(anchor, /data-web-preview="external"/)
    assert.match(anchor, /onClick=/)
  }
  assert.match(
    updates,
    /<a\s+href=\{sponsor\.href\}[\s\S]*?data-web-preview="external"[\s\S]*?onClick=\{openSponsorLink\}/,
  )
  assert.equal(
    shouldOpenWebPreview({
      href: 'https://github.com/ling-kong-ran/pisper',
      baseUrl: 'http://127.0.0.1:5173/#/config/about',
      behavior: 'external',
    }),
    null,
  )
})
