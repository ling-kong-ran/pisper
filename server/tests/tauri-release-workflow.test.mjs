import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
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
})
