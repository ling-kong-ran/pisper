import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('asset UI previews text and video while preserving a web download fallback', async () => {
  const [page, helper] = await Promise.all([
    readFile('src/features/assets/AssetsPage.tsx', 'utf8'),
    readFile('src/lib/open-asset.ts', 'utf8'),
  ])

  assert.match(page, /content\?preview=1/)
  assert.match(page, /<MarkdownMessage>/)
  assert.match(page, /<video[\s\S]*controls[\s\S]*playsInline/)
  assert.match(page, /openAssetInApplication/)
  assert.match(helper, /mobile_open_asset/)
  assert.match(helper, /pisperDesktop\?\.openAsset/)
  assert.match(helper, /anchor\.download = asset\.name/)
})

test('desktop and mobile shells expose only the bounded asset-open bridge', async () => {
  const [
    desktopRust,
    desktopScript,
    desktopPermission,
    mobileRust,
    mobilePermission,
    swift,
    kotlin,
    manifest,
  ] = await Promise.all([
    readFile('src-tauri/src/desktop_shell/desktop_bridge.rs', 'utf8'),
    readFile('src-tauri/src/desktop_shell/desktop-bridge.js', 'utf8'),
    readFile('src-tauri/permissions/desktop.toml', 'utf8'),
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile('src-tauri/permissions/mobile.toml', 'utf8'),
    readFile('src-tauri/mobile-device-plugin/ios/Sources/MobileDevicePlugin.swift', 'utf8'),
    readFile(
      'src-tauri/mobile-device-plugin/android/src/main/java/app/pisper/mobiledevice/MobileDevicePlugin.kt',
      'utf8',
    ),
    readFile('src-tauri/mobile-device-plugin/android/src/main/AndroidManifest.xml', 'utf8'),
  ])

  assert.match(desktopRust, /fn desktop_open_asset/)
  assert.match(desktopRust, /128 \* 1024 \* 1024/)
  assert.match(desktopScript, /desktop_open_asset/)
  assert.match(desktopPermission, /"desktop_open_asset"/)
  assert.match(mobileRust, /fn mobile_open_asset/)
  assert.match(mobileRust, /operation: "files\.open"/)
  assert.match(mobilePermission, /"mobile_open_asset"/)
  assert.match(swift, /case "files\.open": openFile/)
  assert.match(kotlin, /"files\.open" -> openFile/)
  assert.match(manifest, /\.pisperassets/)
  assert.doesNotMatch(mobileRust, /"files\.open"\s*=>\s*Ok\(Some\("externalApps"\)\)/)
})
