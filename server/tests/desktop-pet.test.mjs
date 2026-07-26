import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  MAX_PET_BYTES,
  PET_WINDOW_HEIGHT,
  PET_WINDOW_WIDTH,
  isPetSheetDimensions,
  petBubbleKeyForState,
  petStateForAgentEvent,
  readImageDimensions,
  resolvePetPosition,
} from '../../electron/desktop-pet-state.mjs'
import { WebDesktopPetService } from '../services/web-desktop-pet-service.mjs'
import { fetchAllowedHttps } from '../../electron/petdex-fetch.mjs'

const ROOT = new URL('../../', import.meta.url)

test('Petdex PNG and WebP sheets are validated without a renderer dependency', () => {
  const webp = Buffer.alloc(30)
  webp.write('RIFF', 0, 'ascii')
  webp.write('WEBP', 8, 'ascii')
  webp.write('VP8X', 12, 'ascii')
  webp.writeUIntLE(1535, 24, 3)
  webp.writeUIntLE(2287, 27, 3)
  assert.deepEqual(readImageDimensions(webp), { width: 1536, height: 2288, mime: 'image/webp' })

  const png = Buffer.alloc(24)
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]).copy(png)
  png.writeUInt32BE(1536, 16)
  png.writeUInt32BE(1872, 20)
  assert.deepEqual(readImageDimensions(png), { width: 1536, height: 1872, mime: 'image/png' })
  assert.equal(isPetSheetDimensions({ width: 1536, height: 1872 }), true)
  assert.equal(isPetSheetDimensions({ width: 1536, height: 1800 }), false)
  assert.equal(readImageDimensions(Buffer.from('not-an-image')), null)
})

test('Agent runtime events map to Petdex sprite states', () => {
  assert.equal(petStateForAgentEvent('meta'), 'waiting')
  assert.equal(petStateForAgentEvent('thinking_patch'), 'review')
  assert.equal(petStateForAgentEvent('tool_start'), 'running')
  assert.equal(petStateForAgentEvent('done'), 'waving')
  assert.equal(petStateForAgentEvent('error'), 'failed')
  assert.equal(petStateForAgentEvent('session_title'), null)
  assert.equal(petBubbleKeyForState('jumping'), 'pet.bubbleHello')
})

test('desktop pet position survives valid multi-display coordinates and recovers invalid ones', () => {
  const displays = [
    { workArea: { x: 0, y: 0, width: 1920, height: 1040 } },
    { workArea: { x: 1920, y: 0, width: 1280, height: 1024 } },
  ]
  assert.deepEqual(resolvePetPosition({ x: 2100, y: 600 }, displays, displays[0]), { x: 2100, y: 600 })
  assert.deepEqual(resolvePetPosition({ x: 9000, y: 9000 }, displays, displays[0]), {
    x: 1920 - PET_WINDOW_WIDTH - 20,
    y: 1040 - PET_WINDOW_HEIGHT - 20,
  })
})

test('Electron Petdex fetch follows allowlisted redirects without relying on response.url', async () => {
  const requests = []
  const fetchFn = async (url, options) => {
    requests.push({ url, options })
    if (url === 'https://petdex.dev/api/manifest')
      return new Response(null, {
        status: 302,
        headers: { location: 'https://assets.petdex.dev/manifests/petdex-v1.json' },
      })
    return new Response('{"pets":[]}', { status: 200 })
  }
  const { response, finalUrl } = await fetchAllowedHttps(
    fetchFn,
    'https://petdex.dev/api/manifest',
    {
      allowedHost: 'petdex.dev',
      redirectHosts: ['assets.petdex.dev'],
    },
  )
  assert.equal(response.status, 200)
  assert.equal(finalUrl.href, 'https://assets.petdex.dev/manifests/petdex-v1.json')
  assert.deepEqual(
    requests.map((request) => request.url),
    [
      'https://petdex.dev/api/manifest',
      'https://assets.petdex.dev/manifests/petdex-v1.json',
    ],
  )
  assert.equal(requests[0].options.redirect, 'manual')
  await assert.rejects(
    fetchAllowedHttps(
      async () =>
        new Response(null, {
          status: 302,
          headers: { location: 'https://example.com/manifest.json' },
        }),
      'https://petdex.dev/api/manifest',
      { allowedHost: 'petdex.dev', redirectHosts: ['assets.petdex.dev'] },
    ),
    /UNTRUSTED_URL/,
  )
})

test('Web pet service installs validated resources and publishes Agent state', async () => {
  const dataDir = await mkdtemp(join(tmpdir(), 'vesper-web-pet-'))
  const webp = Buffer.alloc(30)
  webp.write('RIFF', 0, 'ascii')
  webp.write('WEBP', 8, 'ascii')
  webp.write('VP8X', 12, 'ascii')
  webp.writeUIntLE(1535, 24, 3)
  webp.writeUIntLE(1871, 27, 3)
  const manifest = Buffer.from(
    JSON.stringify({
      pets: [
        {
          slug: 'boba',
          displayName: 'Boba',
          spritesheetUrl: 'https://assets.petdex.dev/curated/boba/sprite.webp',
        },
      ],
    }),
  )
  const fetchFn = async (url) => {
    if (url === 'https://petdex.dev/api/manifest')
      return new Response(manifest, { headers: { 'content-type': 'application/json' } })
    if (url === 'https://assets.petdex.dev/curated/boba/sprite.webp')
      return new Response(webp, { headers: { 'content-type': 'image/webp' } })
    return new Response('missing', { status: 404 })
  }
  const service = new WebDesktopPetService({ dataDir, fetchFn })
  try {
    assert.deepEqual(await service.search('bob'), [{ slug: 'boba', displayName: 'Boba' }])
    let status = await service.install('boba')
    assert.equal(status.selectedSlug, 'boba')
    assert.equal(status.running, false)
    status = service.setEnabled(true)
    assert.equal(status.running, true)
    assert.match(status.spriteUrl, /^\/api\/desktop-pet\/sprite\?slug=boba/)
    assert.equal(service.sprite('boba')?.mime, 'image/webp')
    service.observeRuntimeEvent({ event: 'thinking_patch', sessionId: 'session-1' })
    assert.equal(service.status().state, 'review')
    service.observeRuntimeEvent({ event: 'done', sessionId: 'session-1' })
    assert.equal(service.status().state, 'waving')
  } finally {
    service.dispose()
    await rm(dataDir, { recursive: true, force: true })
  }
})

test('Electron pet integration remains independent from the hidden main window', async () => {
  const [main, html, petPreload, appPreload, settings, configPage, webPet, appShell] = await Promise.all([
    readFile(new URL('electron/main.mjs', ROOT), 'utf8'),
    readFile(new URL('electron/pet-window.html', ROOT), 'utf8'),
    readFile(new URL('electron/pet-preload.cjs', ROOT), 'utf8'),
    readFile(new URL('electron/preload.cjs', ROOT), 'utf8'),
    readFile(new URL('src/features/config/DesktopPetSettings.tsx', ROOT), 'utf8'),
    readFile(new URL('src/features/config/ConfigPage.tsx', ROOT), 'utf8'),
    readFile(new URL('src/features/desktop-pet/WebDesktopPet.tsx', ROOT), 'utf8'),
    readFile(new URL('src/App.tsx', ROOT), 'utf8'),
  ])

  assert.match(main, /let petWindow = null/)
  assert.match(main, /mainWindow\.hide\(\)/)
  assert.match(main, /skipTaskbar: true/)
  assert.match(main, /alwaysOnTop: true/)
  assert.match(main, /showInactive\(\)/)
  assert.match(main, /runtimeEventObserver: observeRuntimeEvent/)
  assert.match(main, /'desktop-pets'/)
  assert.match(main, /PETDEX_MANIFEST_URL/)
  assert.match(main, /fetchAllowedHttps\(globalThis\.fetch/)
  assert.doesNotMatch(main, /net\.fetch\(currentUrl\.href/)
  assert.match(main, /ipcMain\.handle\('vesper:install-pet'/)
  assert.doesNotMatch(main, /petdex desktop start|spawn_sidecar|npx petdex/)
  assert.equal(MAX_PET_BYTES, 16 * 1024 * 1024)
  assert.match(html, /Content-Security-Policy/)
  assert.match(petPreload, /contextBridge\.exposeInMainWorld\('vesperPet'/)
  assert.match(appPreload, /installPet: \(slug\)/)
  assert.match(settings, /Vesper desktop pet|desktopPetSettings/)
  assert.match(configPage, /section === 'desktop-pet'/)
  assert.doesNotMatch(configPage, /vesperDesktop\?\.getPetStatus &&/)
  assert.match(webPet, /position:|web-desktop-pet|onPointerMove/)
  assert.match(appShell, /<WebDesktopPet \/>/)
})
