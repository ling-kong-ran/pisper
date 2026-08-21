// 移动 App 更新门禁：独立 app-v 版本必须同时进入原生包、发布清单与移动桥权限。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { isAppExclusivePath, isAppOwnedPath } from '../../scripts/app-paths.mjs'

test('移动 App 更新实现归独立 App 发布通道', () => {
  const path = 'src-tauri/src/mobile/update.rs'
  assert.equal(isAppExclusivePath(path), true)
  assert.equal(isAppOwnedPath(path), true)
})

test('Android 与 iOS 发布包都写入独立 App 版本', async () => {
  const [workflow, localBuild] = await Promise.all([
    readFile('.github/workflows/release-app.yml', 'utf8'),
    readFile('scripts/build-mobile-android.mjs', 'utf8'),
  ])

  assert.match(workflow, /android build[\s\S]*--config '\{"version":"\$\{\{ inputs\.version \}\}"/)
  assert.match(workflow, /ios build[\s\S]*--config '\{"version":"\$\{\{ inputs\.version \}\}"/)
  assert.match(localBuild, /mobile-package\.json/)
  assert.match(localBuild, /version: mobileVersion/)
})

test('App Release 仅在 Android 与 iOS 产物都成功时发布', async () => {
  const workflow = await readFile('.github/workflows/release-app.yml', 'utf8')
  assert.match(workflow, /needs\.ios\.result == 'success'/)
  assert.doesNotMatch(workflow, /name: Build iOS app[^\n]*best-effort/)
  for (const asset of [
    'artifacts/pisper-ios-unsigned.ipa',
    'artifacts/pisper-ios-unsigned.ipa.minisig',
    'artifacts/README-ios.txt',
  ]) {
    assert.match(workflow, new RegExp(`test -f ${asset.replaceAll('.', '\\.')}`))
  }
})

test('App manifest 提供平台资产、更新说明与发布日期', async () => {
  const prepare = await readFile('scripts/prepare-app-release.mjs', 'utf8')
  for (const field of [
    "apk: 'app-universal-release-signed.apk'",
    "ipa: 'pisper-ios-unsigned.ipa'",
  ]) {
    assert.match(prepare, new RegExp(field.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))
  }
  assert.match(prepare, /notes: subjects/)
  assert.match(prepare, /releaseDate: date/)
})

test('移动桥只开放受控的 App 更新命令', async () => {
  const [permissions, implementation] = await Promise.all([
    readFile('src-tauri/permissions/mobile.toml', 'utf8'),
    readFile('src-tauri/src/mobile/update.rs', 'utf8'),
  ])
  for (const command of ['mobile_app_info', 'mobile_check_app_update', 'mobile_open_app_update']) {
    assert.match(permissions, new RegExp(`"${command}"`))
  }
  assert.match(implementation, /https:\/\/ling-kong-ran\.github\.io\/pisper\/latest-app\.json/)
  assert.match(implementation, /github\.com\/ling-kong-ran\/pisper\/releases/)
  assert.match(implementation, /AUTOMATIC_CHECK_INTERVAL/)
})
