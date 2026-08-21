// 移动端发布资产门禁：图标必须由桌面 ICNS 同步，并在平台工程初始化后覆盖模板图标。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { isAppExclusivePath, isAppOwnedPath } from '../../scripts/app-paths.mjs'
import { releaseComponentsForPath } from '../../scripts/release-changes.mjs'

function pngSize(buffer) {
  assert.equal(buffer.subarray(1, 4).toString('ascii'), 'PNG')
  return [buffer.readUInt32BE(16), buffer.readUInt32BE(20)]
}

test('Android 与 iOS 图标由桌面品牌图标生成', async () => {
  const [script, androidIcon, iosIcon] = await Promise.all([
    readFile('scripts/sync-mobile-icons.mjs', 'utf8'),
    readFile('src-tauri/icons/android/mipmap-xxxhdpi/ic_launcher.png'),
    readFile('src-tauri/icons/ios/AppIcon-512@2x.png'),
  ])
  assert.match(script, /src-tauri["'], ["']icons/)
  assert.match(script, /icon\.icns/)
  assert.match(script, /extractIcnsPng\(readFileSync\(icnsPath\), 'ic10'\)/)
  assert.deepEqual(pngSize(androidIcon), [192, 192])
  assert.deepEqual(pngSize(iosIcon), [1024, 1024])
  assert.equal(isAppOwnedPath('src-tauri/icons/android/mipmap-mdpi/ic_launcher.png'), true)
  assert.equal(isAppOwnedPath('src-tauri/icons/ios/AppIcon-512@2x.png'), true)
})

test('共享 Rust 路径同时归 App 与 Desktop 发布通道', () => {
  for (const path of [
    'src-tauri/Cargo.lock',
    'src-tauri/Cargo.toml',
    'src-tauri/src/iroh_tunnel.rs',
    'src-tauri/src/lib.rs',
  ]) {
    assert.equal(isAppOwnedPath(path), true)
    assert.equal(isAppExclusivePath(path), false)
    assert.deepEqual(releaseComponentsForPath(path), ['desktop'])
  }
  assert.equal(isAppExclusivePath('src-tauri/src/mobile/proxy.rs'), true)
  assert.deepEqual(releaseComponentsForPath('src-tauri/src/mobile/proxy.rs'), [])
})

test('平台初始化流程不会保留 Tauri 模板图标', async () => {
  const [androidSetup, workflow] = await Promise.all([
    readFile('scripts/setup-mobile-android.mjs', 'utf8'),
    readFile('.github/workflows/release-app.yml', 'utf8'),
  ])
  const androidInit = androidSetup.indexOf("'android', 'init'")
  const androidIcons = androidSetup.indexOf("'sync-mobile-icons.mjs'")
  assert.ok(androidInit >= 0 && androidIcons > androidInit)

  const iosIcons = workflow.indexOf('node scripts/sync-mobile-icons.mjs')
  const iosInit = workflow.indexOf('npx tauri ios init')
  assert.ok(iosIcons >= 0 && iosInit > iosIcons)
})
