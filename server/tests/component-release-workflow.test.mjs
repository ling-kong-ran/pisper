import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { access, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  fallbackReleaseTag,
  readComponentVersion,
  releaseTag,
  releaseVersionFromTag,
} from '../../scripts/release-components.mjs'

test('desktop, TUI, and server own independent versions and tags', async () => {
  const root = process.cwd()
  const [desktop, tui, server] = await Promise.all([
    readComponentVersion(root, 'desktop'),
    readComponentVersion(root, 'tui'),
    readComponentVersion(root, 'server'),
  ])

  for (const version of [desktop, tui, server]) {
    assert.match(version, /^\d+\.\d+\.\d+$/)
  }
  assert.equal(releaseTag('desktop', '1.2.3'), 'v1.2.3')
  assert.equal(releaseTag('tui', '1.2.3'), 'tui-v1.2.3')
  assert.equal(releaseTag('server', '1.2.3'), 'server-v1.2.3')
  assert.equal(releaseVersionFromTag('tui', 'tui-v1.2.3'), '1.2.3')
  assert.equal(releaseVersionFromTag('server', 'v1.2.3'), '')
})

test('new component channels migrate from the latest legacy desktop tag', () => {
  const tags = ['server-v0.4.20', 'tui-v0.4.22', 'v0.4.26', 'v0.4.25']
  assert.equal(fallbackReleaseTag('desktop', tags), 'v0.4.26')
  assert.equal(fallbackReleaseTag('tui', tags), 'tui-v0.4.22')
  assert.equal(fallbackReleaseTag('server', tags), 'server-v0.4.20')
  assert.equal(fallbackReleaseTag('tui', ['v0.4.26', 'v0.4.25']), 'v0.4.26')
})

test('Tauri reads only the desktop version while component packagers keep their own versions', async () => {
  const [configSource, tauriPackager, tuiPackager, cliStager] = await Promise.all([
    readFile('src-tauri/tauri.conf.json', 'utf8'),
    readFile('scripts/package-tauri-release.mjs', 'utf8'),
    readFile('scripts/package-tui.mjs', 'utf8'),
    readFile('scripts/stage-tauri-cli.mjs', 'utf8'),
  ])
  const config = JSON.parse(configSource)

  assert.equal(config.version, 'desktop-package.json')
  assert.match(tauriPackager, /desktop-package\.json/)
  assert.doesNotMatch(tauriPackager, /version from \.\.\/package\.json/)
  assert.match(tuiPackager, /const tuiVersion/)
  assert.match(tuiPackager, /serverVersion: packageJson\.version/)
  assert.doesNotMatch(tuiPackager, /sync-tui-version/)
  assert.doesNotMatch(cliStager, /sync-tui-version/)
  await assert.rejects(access('scripts/sync-tui-version.mjs'), (error) => error?.code === 'ENOENT')
})

test('release workflow builds and finalizes only the selected component channel', async () => {
  const workflow = await readFile('.github/workflows/release.yml', 'utf8')

  assert.match(workflow, /options:\s*\n\s*- desktop\s*\n\s*- tui\s*\n\s*- server/)
  assert.match(workflow, /if: inputs\.component == 'desktop'/)
  assert.match(workflow, /if: inputs\.component == 'tui'/)
  assert.match(workflow, /if: inputs\.component == 'server'/)
  assert.match(workflow, /node scripts\/archive-component-release\.mjs tui/)
  assert.match(workflow, /node scripts\/archive-component-release\.mjs server/)
  assert.match(workflow, /node scripts\/validate-component-release-assets\.mjs/)
  assert.match(workflow, /--latest="\$MAKE_LATEST"/)
  assert.match(workflow, /MAKE_LATEST: .*inputs\.component == 'desktop'/)
  assert.match(workflow, /desktop\) git add src-tauri\/desktop-package\.json/)
  assert.match(workflow, /tui\) git add src-tui\/Cargo\.toml src-tui\/Cargo\.lock/)
  assert.match(workflow, /server\) git add package\.json package-lock\.json/)
})

test('release command selects scoped local checks and dispatch input', async () => {
  const source = await readFile('scripts/release.mjs', 'utf8')

  assert.match(source, /RELEASE_COMPONENTS\[args\[0\]\] \? args\.shift\(\) : 'desktop'/)
  assert.match(source, /if \(selected === 'server'\)/)
  assert.match(source, /if \(selected === 'desktop'\)/)
  assert.match(source, /`component=\$\{component\}`/)
  assert.match(source, /只会执行 \$\{component\} 对应的质量门禁和平台产物构建/)
})

test('component release validator requires exactly four platform archives', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-component-assets-'))
  const expected = [
    'Pisper_TUI_1.2.3_darwin_aarch64.tar.gz',
    'Pisper_TUI_1.2.3_darwin_x86_64.tar.gz',
    'Pisper_TUI_1.2.3_linux_x86_64.tar.gz',
    'Pisper_TUI_1.2.3_windows_x86_64.tar.gz',
  ]

  try {
    await Promise.all(expected.map((name) => writeFile(join(directory, name), 'artifact')))
    const valid = spawnSync(
      process.execPath,
      ['scripts/validate-component-release-assets.mjs', 'tui', 'tui-v1.2.3', directory],
      { encoding: 'utf8' },
    )
    assert.equal(valid.status, 0, valid.stderr)

    await writeFile(join(directory, 'unexpected.txt'), 'artifact')
    const invalid = spawnSync(
      process.execPath,
      ['scripts/validate-component-release-assets.mjs', 'tui', 'tui-v1.2.3', directory],
      { encoding: 'utf8' },
    )
    assert.notEqual(invalid.status, 0)
    assert.match(invalid.stderr, /Unexpected release assets: unexpected\.txt/)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})
