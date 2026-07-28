import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

test('transparent desktop pet enables the required macOS Tauri API', async () => {
  const [cargo, config, desktopPet] = await Promise.all([
    readFile('src-tauri/Cargo.toml', 'utf8'),
    readFile('src-tauri/tauri.conf.json', 'utf8'),
    readFile('src-tauri/src/desktop_pet.rs', 'utf8'),
  ])

  assert.match(desktopPet, /\.transparent\(true\)/)
  assert.match(cargo, /features = \["tray-icon", "macos-private-api"\]/)
  assert.equal(JSON.parse(config).app.macOSPrivateApi, true)
})

test('release quality builds the external SEA sidecar before checking Rust', async () => {
  const workflow = await readFile('.github/workflows/release.yml', 'utf8')
  const qualityJob = workflow.slice(workflow.indexOf('quality:'), workflow.indexOf('  build:'))

  assert.ok(qualityJob.indexOf('npm run sidecar:sea') < qualityJob.indexOf('cargo check'))
})

test('v0.4.0 alone republishes the archived Electron transition assets', async () => {
  const workflow = await readFile('.github/workflows/release.yml', 'utf8')
  const transitionStep = workflow.slice(
    workflow.indexOf('Stage Electron transition assets for v0.4.0'),
    workflow.indexOf('Create multi-platform updater manifest'),
  )

  assert.match(transitionStep, /if: github\.ref_name == 'v0\.4\.0'/)
  assert.match(transitionStep, /gh release download v0\.3\.3/)
  assert.match(transitionStep, /--dir artifacts\/electron-transition/)
  assert.doesNotMatch(workflow, /if: startsWith\(github\.ref_name, 'v0\.4'/)
  assert.match(workflow, /node scripts\/validate-tauri-release-assets\.mjs/)
})

test('v0.4.1 release assets reject Electron metadata and unexpected files', async () => {
  const version = '0.4.1'
  const directory = await mkdtemp(join(tmpdir(), 'pisper-release-assets-'))
  const expected = [
    'latest.json',
    `Pisper_${version}_darwin_aarch64.app.tar.gz`,
    `Pisper_${version}_darwin_aarch64.app.tar.gz.sig`,
    `Pisper_${version}_darwin_aarch64.dmg`,
    `Pisper_${version}_darwin_x86_64.app.tar.gz`,
    `Pisper_${version}_darwin_x86_64.app.tar.gz.sig`,
    `Pisper_${version}_darwin_x86_64.dmg`,
    `Pisper_${version}_linux_x86_64.AppImage`,
    `Pisper_${version}_linux_x86_64.AppImage.sig`,
    `Pisper_${version}_linux_x86_64.deb`,
    `Pisper_${version}_windows_x86_64-setup.exe`,
    `Pisper_${version}_windows_x86_64-setup.exe.sig`,
  ]

  try {
    await Promise.all(expected.map((name) => writeFile(join(directory, name), 'artifact')))
    const valid = spawnSync(
      process.execPath,
      ['scripts/validate-tauri-release-assets.mjs', `v${version}`, directory],
      { encoding: 'utf8' },
    )
    assert.equal(valid.status, 0, valid.stderr)

    await writeFile(join(directory, 'latest.yml'), 'version: 0.3.3')
    const invalid = spawnSync(
      process.execPath,
      ['scripts/validate-tauri-release-assets.mjs', `v${version}`, directory],
      { encoding: 'utf8' },
    )
    assert.notEqual(invalid.status, 0)
    assert.match(invalid.stderr, /Unexpected release assets: latest\.yml/)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})
