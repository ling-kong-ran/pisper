import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  PET_WINDOW_HEIGHT,
  PET_WINDOW_WIDTH,
  isPetSheetDimensions,
  normalizePetOpacity,
  petBubbleKeyForState,
  petStateForAgentEvent,
  readImageDimensions,
  resolvePetPosition,
} from '../../shared/desktop-pet-state.mjs'
import { WebDesktopPetService } from '../services/web-desktop-pet-service.mjs'

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
  assert.equal(normalizePetOpacity(0), 0.2)
  assert.equal(normalizePetOpacity(0.65), 0.65)
  assert.equal(normalizePetOpacity(2), 1)
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
  assert.deepEqual(resolvePetPosition({ x: 2100, y: 600 }, displays, displays[0]), {
    x: 2100,
    y: 600,
  })
  const lowerRight = {
    x: 1920 - PET_WINDOW_WIDTH - 20,
    y: 1040 - PET_WINDOW_HEIGHT - 20,
  }
  assert.deepEqual(resolvePetPosition({ x: 9000, y: 9000 }, displays, displays[0]), lowerRight)
  assert.deepEqual(resolvePetPosition({ x: 0, y: 0 }, displays, displays[0]), lowerRight)
  assert.deepEqual(resolvePetPosition({ x: 0, y: 0, customized: true }, displays, displays[0]), {
    x: 0,
    y: 0,
  })
})

test('Web pet service installs validated resources and publishes Agent state', async () => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-web-pet-'))
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
    status = service.setOpacity(0.55)
    assert.equal(status.opacity, 0.55)
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

test('Tauri pet and tray menus preserve desktop pet controls', async () => {
  const [shell, petShell, desktopBridge, petRenderer, permissions] = await Promise.all([
    readFile(new URL('src-tauri/src/lib.rs', ROOT), 'utf8'),
    readFile(new URL('src-tauri/src/desktop_pet.rs', ROOT), 'utf8'),
    readFile(new URL('src-tauri/src/desktop-bridge.js', ROOT), 'utf8'),
    readFile(new URL('public/tauri-pet.js', ROOT), 'utf8'),
    readFile(new URL('src-tauri/permissions/desktop.toml', ROOT), 'utf8'),
  ])

  assert.match(shell, /CheckMenuItem::with_id\(app, "tray_pet"/)
  assert.match(shell, /"pet_hide" => request_desktop_pet_enabled\(app, false\)/)
  assert.match(shell, /window\.popup_menu\(&state\.menu\)/)
  assert.match(shell, /if ready\.desktop_pet_running/)
  assert.match(shell, /desktop_pet::show_pet_window\(app\.handle\(\)\)/)
  assert.match(petShell, /pub\(crate\) fn show_pet_window/)
  assert.match(petShell, /window\.show\(\)/)
  assert.match(petShell, /window\.unminimize\(\)/)
  assert.match(petShell, /desktop_pet_apply_enabled/)
  assert.match(petShell, /window\.destroy\(\)/)
  assert.match(desktopBridge, /desktop_pet_apply_enabled/)
  assert.match(petRenderer, /invoke\('desktop_pet_show_context_menu'\)/)
  assert.match(petRenderer, /fetch\('\/api\/desktop-pet\/enabled'/)
  assert.match(petRenderer, /__PISPER_DESKTOP_PET_SET_ENABLED/)
  assert.match(permissions, /"desktop_pet_apply_enabled"/)
  assert.match(permissions, /"desktop_pet_show_context_menu"/)
  assert.match(permissions, /"desktop_pet_sync_menu"/)
})
