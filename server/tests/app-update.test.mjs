import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  newerVersion,
  preferredUpdateVersion,
  reconcileDesktopUpdateCheck,
  versionAtLeast,
} from '../../shared/app-update.mjs'

test('Electron updater prompts users to install Tauri releases manually', async () => {
  const [main, zh, en] = await Promise.all([
    readFile(new URL('../../electron/main.mjs', import.meta.url), 'utf8'),
    readFile(new URL('../../electron/locales/zh-CN.json', import.meta.url), 'utf8'),
    readFile(new URL('../../electron/locales/en-US.json', import.meta.url), 'utf8'),
  ])
  assert.match(main, /TAURI_MIGRATION_VERSION = '0\.4\.0'/)
  assert.match(main, /await promptTauriMigration\(latest\)/)
  assert.match(main, /canDownload: false/)
  assert.match(zh, /下载最新 Tauri 版/)
  assert.match(en, /Download latest Tauri version/)
})

test('Tauri migration threshold starts at v0.4.0', () => {
  assert.equal(versionAtLeast('0.3.3', '0.4.0'), false)
  assert.equal(versionAtLeast('v0.4.0', '0.4.0'), true)
  assert.equal(versionAtLeast('0.4.1', '0.4.0'), true)
})

test('preferred update version keeps the higher of GitHub and updater metadata', () => {
  assert.equal(preferredUpdateVersion('0.2.0', '0.1.3'), '0.2.0')
  assert.equal(preferredUpdateVersion('0.1.3', '0.2.0'), '0.2.0')
  assert.equal(preferredUpdateVersion('v0.2.0', ''), '0.2.0')
  assert.equal(preferredUpdateVersion('', '0.1.3'), '0.1.3')
})

test('desktop update check prefers GitHub when electron-updater metadata is stale', () => {
  const result = reconcileDesktopUpdateCheck({
    appVersion: '0.1.2',
    githubVersion: '0.2.0',
    githubNotes: '## 0.2.0',
    githubReleaseUrl: 'https://github.com/ling-kong-ran/pisper/releases/tag/v0.2.0',
    updaterVersion: '0.1.3',
    updaterIsAvailable: true,
    previousState: 'available',
    previousAvailableVersion: '0.1.3',
  })
  assert.equal(result.state, 'available')
  assert.equal(result.availableVersion, '0.2.0')
  assert.equal(result.canDownload, false)
  assert.match(result.message, /元数据尚未同步|GitHub Releases/)
})

test('desktop update check allows download only when updater metadata matches GitHub', () => {
  const result = reconcileDesktopUpdateCheck({
    appVersion: '0.1.3',
    githubVersion: '0.2.0',
    updaterVersion: '0.2.0',
    updaterIsAvailable: true,
  })
  assert.equal(result.state, 'available')
  assert.equal(result.availableVersion, '0.2.0')
  assert.equal(result.canDownload, true)
  assert.equal(result.message, '')
})

test('desktop update check does not keep a downloaded older package when a newer release exists', () => {
  assert.equal(newerVersion('0.2.0', '0.1.3'), true)
  const result = reconcileDesktopUpdateCheck({
    appVersion: '0.1.2',
    githubVersion: '0.2.0',
    updaterVersion: '0.2.0',
    updaterIsAvailable: true,
    previousState: 'downloaded',
    previousAvailableVersion: '0.1.3',
  })
  assert.equal(result.state, 'available')
  assert.equal(result.availableVersion, '0.2.0')
  assert.equal(result.canInstall, false)
  assert.equal(result.canDownload, true)
})
