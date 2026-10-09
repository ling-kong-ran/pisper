import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { copyFile, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const repo = fileURLToPath(new URL('../../', import.meta.url))
const preload = new URL('./fixtures/release-command-fixture.mjs', import.meta.url).href
const sourceSha = '1111111111111111111111111111111111111111'

async function runRelease(overrides = {}) {
  const fixture = {
    layout: 'rust',
    paths: ['src/project.tsx', 'src-tui/src/main.rs', 'runtime-rs/src/main.rs', 'package.json'],
    version: '0.6.0',
    ...overrides,
  }
  const root = await mkdtemp(path.join(tmpdir(), 'pisper-release-command-'))
  try {
    async function put(relative, contents) {
      const target = path.join(root, relative)
      await mkdir(path.dirname(target), { recursive: true })
      await writeFile(target, contents)
    }
    await mkdir(path.join(root, 'scripts'))
    for (const name of [
      'release.mjs',
      'release-changes.mjs',
      'release-components.mjs',
      'release-policy.mjs',
      'app-paths.mjs',
    ]) {
      await copyFile(path.join(repo, 'scripts', name), path.join(root, 'scripts', name))
    }
    await put('package.json', JSON.stringify({ version: '0.5.62' }))
    await put(
      'src-tauri/desktop-package.json',
      JSON.stringify({ version: '0.5.81', bundled: { tui: '0.5.38', runtime: '0.5.62' } }),
    )
    await put('src-tauri/mobile-package.json', JSON.stringify({ version: '0.1.55' }))
    await put('src-tui/Cargo.toml', '[package]\nname = "pisper-tui"\nversion = "0.5.38"\n')
    await put(
      'packages/pisper/package.json',
      JSON.stringify({
        version: '0.5.80',
        pisper: { tuiVersion: '0.5.38', runtimeVersion: '0.5.62' },
      }),
    )
    if (fixture.layout === 'rust') await put('runtime-rs/Cargo.toml', '[workspace]\n')
    else await put('runtime/index.mjs', 'export {}\n')
    const result = spawnSync(
      process.execPath,
      ['--import', preload, path.join(root, 'scripts/release.mjs'), fixture.version],
      {
        cwd: root,
        encoding: 'utf8',
        env: {
          ...process.env,
          PISPER_RELEASE_BRANCH: 'release',
          PISPER_RELEASE_TEST_FIXTURE: JSON.stringify(fixture),
          npm_execpath: 'fixture-npm-cli',
        },
      },
    )
    if (result.error) throw result.error
    const trace = result.stdout.match(/^RELEASE_TEST_COMMANDS=(.+)$/m)
    assert.ok(trace, result.stderr || result.stdout)
    return { ...result, commands: JSON.parse(trace[1]) }
  } finally {
    // root 只来自本测试的 mkdtemp，清理不会触及仓库或真实发布产物。
    await rm(root, { recursive: true, force: true })
  }
}

function dispatches(result) {
  return result.commands.filter(({ command, args }) => command === 'gh' && args[0] === 'workflow')
}

test('Rust release dispatches only the Windows desktop workflow without updating Node Pi', async () => {
  const result = await runRelease()
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(
    dispatches(result).map(({ args }) => args),
    [
      [
        'workflow',
        'run',
        'release.yml',
        '--ref',
        'release',
        '-f',
        'component=desktop',
        '-f',
        'version=0.6.0',
        '-f',
        `source_sha=${sourceSha}`,
      ],
    ],
  )
  assert.equal(
    result.commands.some(({ args }) => args.includes('install')),
    false,
  )
  const npmCommands = result.commands
    .filter(({ args }) => args[0] === 'fixture-npm-cli')
    .map(({ args }) => args.slice(1).join(' '))
  assert.ok(npmCommands.includes('test'))
  assert.ok(npmCommands.includes('run check'))
  assert.ok(npmCommands.includes('run build'))
  assert.equal(npmCommands.includes('run npm:pack:check'), false)
  assert.ok(
    result.commands.some(
      ({ command, args }) => command === 'gh' && args.join(' ') === 'run watch 100 --exit-status',
    ),
  )
})

test('a TUI-only change also releases the desktop installer that bundles it', async () => {
  const result = await runRelease({ paths: ['src-tui/src/main.rs'] })
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(
    dispatches(result).map(({ args }) => args.find((value) => value.startsWith('component='))),
    ['component=desktop'],
  )
})

test('legacy Node layout retains channel order, npm chaining and the dependency update', async () => {
  const result = await runRelease({ layout: 'node' })
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(
    dispatches(result).map(
      ({ args }) => args.find((value) => value.startsWith('component=')) || args[2],
    ),
    ['component=tui', 'component=runtime', 'component=desktop', 'release-app.yml'],
  )
  assert.equal(
    result.commands.some(({ args }) => args.includes('install')),
    true,
  )
  assert.deepEqual(
    dispatches(result)
      .filter(({ args }) => args.includes('publish_npm=true'))
      .map(({ args }) => args.find((value) => value.startsWith('component='))),
    ['component=runtime'],
  )
  for (const { args } of dispatches(result)) assert.ok(args.includes(`source_sha=${sourceSha}`))
})

for (const [name, fixture, message] of [
  ['dirty tracked files', { dirty: true }, /tracked 工作区必须保持干净/],
  ['wrong branch', { branch: 'feature' }, /只能从 release 分支发布/],
  ['remote divergence', { remoteSha: '2222222222222222222222222222222222222222' }, /完全同步/],
  ['existing tag', { existingTag: 'v0.6.0' }, /标签 v0.6.0 已经存在/],
  ['non-increasing version', { version: '0.5.81' }, /必须高于当前 desktop 版本/],
  ['no product changes', { paths: [] }, /未检测到/],
  ['metadata-only commits', { subject: 'chore(release): metadata' }, /未检测到/],
  ['unsupported mobile-only changes', { paths: ['src-tauri/src/mobile/lib.rs'] }, /未检测到/],
]) {
  test(`Rust release rejects ${name} before dispatch`, async () => {
    const result = await runRelease(fixture)
    assert.notEqual(result.status, 0)
    assert.match(result.stderr, message)
    assert.deepEqual(dispatches(result), [])
  })
}
