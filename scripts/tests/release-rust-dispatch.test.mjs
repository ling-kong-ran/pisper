import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
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
    await put(
      'src-tui/Cargo.toml',
      `[package]\nname = "pisper-tui"\nversion = "${fixture.tuiVersion || '0.5.38'}"\n`,
    )
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
          PISPER_RELEASE_POLL_SECONDS: fixture.pollSeconds || '',
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

test('Rust release dispatches native components in dependency order without updating Node Pi', async () => {
  const result = await runRelease()
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(
    dispatches(result).map(({ args }) => args.find((v) => v.startsWith('component=')) || args[2]),
    ['component=tui', 'component=runtime', 'component=desktop', 'release-app.yml'],
  )
  assert.ok(
    dispatches(result)
      .find(({ args }) => args.includes('component=runtime'))
      .args.includes('publish_npm=true'),
  )
  for (const { args } of dispatches(result)) assert.ok(args.includes(`source_sha=${sourceSha}`))
  assert.equal(
    result.commands.some(({ args }) => args.includes('install')),
    false,
  )
  for (const script of ['scripts/update-pi-rust.mjs', 'scripts/update-pi-model-catalog.mjs'])
    assert(result.commands.some(({ args }) => args.includes(script)))
  const npmCommands = result.commands
    .filter(({ args }) => args[0] === 'fixture-npm-cli')
    .map(({ args }) => args.slice(1).join(' '))
  assert.ok(npmCommands.includes('test'))
  assert.ok(npmCommands.includes('run check'))
  assert.ok(npmCommands.includes('run build'))
  assert.equal(npmCommands.includes('run npm:pack:check'), true)
  assert.ok(
    result.commands.some(
      ({ command, args }) => command === 'gh' && args.join(' ') === 'run watch 100 --exit-status',
    ),
  )
})

test('a configured release poll interval applies to each frozen channel watch', async () => {
  const result = await runRelease({ pollSeconds: '600' })
  assert.equal(result.status, 0, result.stderr)
  const watches = result.commands.filter(
    ({ command, args }) => command === 'gh' && args[0] === 'run' && args[1] === 'watch',
  )
  assert.equal(watches.length, dispatches(result).length)
  for (const { args } of watches) {
    assert.ok(args.includes('--interval'))
    assert.equal(args[args.indexOf('--interval') + 1], '600')
    assert.ok(args.includes('--exit-status'))
  }
})

test('a zero poll interval is rejected before release commands can run', async () => {
  const result = await runRelease({ pollSeconds: '0' })
  assert.notEqual(result.status, 0)
  assert.match(result.stderr, /PISPER_RELEASE_POLL_SECONDS/)
  assert.equal(result.commands.length, 0)
})

test('catalog changes are checked before committing and all channels use the frozen updated source', async () => {
  const result = await runRelease({ catalogChanged: true, paths: [] })
  assert.equal(result.status, 0, result.stderr)
  const check = result.commands.findIndex(
    ({ args }) => args.includes('fixture-npm-cli') && args.includes('check'),
  )
  const push = result.commands.findIndex(
    ({ command, args }) => command === 'git' && args[0] === 'push',
  )
  assert(check >= 0 && push > check)
  for (const dispatch of dispatches(result))
    assert(dispatch.args.includes('source_sha=3333333333333333333333333333333333333333'))
  assert(dispatches(result).some(({ args }) => args[2] === 'release-app.yml'))
})

test('upstream changes normalized away by Git do not create an empty commit or rerun checks', async () => {
  const result = await runRelease({
    catalogChanged: true,
    catalogIndexEmpty: true,
    paths: ['src/project.tsx'],
  })
  assert.equal(result.status, 0, result.stderr)
  assert.equal(
    result.commands.some(
      ({ command, args }) => command === 'git' && ['commit', 'push'].includes(args[0]),
    ),
    false,
  )
  for (const check of ['check', 'test:release']) {
    assert.equal(
      result.commands.filter(({ args }) => args.includes('fixture-npm-cli') && args.includes(check))
        .length,
      1,
    )
  }
  assert.equal(dispatches(result).length, 2)
  for (const dispatch of dispatches(result))
    assert(dispatch.args.includes(`source_sha=${sourceSha}`))
})

for (const fixture of [{ catalogFailure: true }, { catalogChanged: true, checkFailure: true }]) {
  test(`upstream update failure stops before source push and dispatch: ${JSON.stringify(fixture)}`, async () => {
    const result = await runRelease(fixture)
    assert.notEqual(result.status, 0)
    assert.deepEqual(dispatches(result), [])
    assert(
      !result.commands.some(
        ({ command, args }) => command === 'git' && ['commit', 'push'].includes(args[0]),
      ),
    )
  })
}

test('a TUI-only change also releases the desktop installer that bundles it', async () => {
  const result = await runRelease({ paths: ['src-tui/src/main.rs'] })
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(
    dispatches(result).map(({ args }) => args.find((value) => value.startsWith('component='))),
    ['component=tui', 'component=desktop'],
  )
})

test('resuming after TUI publication pins the published version in npm and Desktop without republishing TUI', async () => {
  const result = await runRelease({
    version: '0.6.1',
    tuiVersion: '0.6.1',
    paths: ['runtime-rs/src/main.rs', 'src/project.tsx'],
    tags: ['tui-v0.5.38', 'v0.5.81', 'tui-v0.6.1', 'runtime-v0.5.62', 'app-v0.1.55'],
  })
  assert.equal(result.status, 0, result.stderr)
  const published = dispatches(result)
  assert.deepEqual(
    published.map(({ args }) => args.find((value) => value.startsWith('component=')) || args[2]),
    ['component=runtime', 'component=desktop', 'release-app.yml'],
  )
  for (const { args } of published.filter(({ args }) => args[2] === 'release.yml')) {
    assert(args.includes('tui_version=0.6.1'))
    assert(args.includes('runtime_version=0.6.1'))
  }
  assert(published[0].args.includes('publish_npm=true'))
})

test('single-component publication retains the newest published sibling instead of stale npm bindings', async () => {
  const result = await runRelease({
    version: '0.6.1',
    paths: ['src-tui/src/main.rs'],
    tags: ['runtime-v0.6.0', 'v0.5.81', 'tui-v0.5.38', 'app-v0.1.55'],
  })
  assert.equal(result.status, 0, result.stderr)
  for (const { args } of dispatches(result)) {
    assert(args.includes('tui_version=0.6.1'))
    assert(args.includes('runtime_version=0.6.0'))
  }
})

test('legacy Desktop tags never substitute for an independently published sibling component', async () => {
  const result = await runRelease({
    version: '0.6.1',
    paths: ['src-tui/src/main.rs'],
    tags: ['v0.5.81', 'tui-v0.5.38', 'app-v0.1.55'],
  })
  assert.equal(result.status, 0, result.stderr)
  const npmDispatch = dispatches(result).find(({ args }) => args.includes('publish_npm=true'))
  assert(npmDispatch.args.includes('runtime_version=0.5.62'))
  assert(!npmDispatch.args.includes('runtime_version=0.5.81'))
})

for (const unavailable of ['draftRelease', 'prerelease', 'missingRelease']) {
  test(`resumed release refuses an unavailable sibling component: ${unavailable}`, async () => {
    const result = await runRelease({
      version: '0.6.1',
      tuiVersion: '0.6.1',
      paths: ['runtime-rs/src/main.rs'],
      tags: ['tui-v0.6.1', 'v0.5.81', 'runtime-v0.5.62', 'app-v0.1.55'],
      [unavailable]: 'tui-v0.6.1',
    })
    assert.notEqual(result.status, 0)
    assert.deepEqual(dispatches(result), [])
  })
}

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
  ['active release workflow', { activeRelease: true }, /冻结的发布源/],
  ['existing tag', { existingTag: 'v0.6.0' }, /标签 v0.6.0 已经存在/],
  ['non-increasing version', { version: '0.5.81' }, /必须高于当前 desktop 版本/],
  ['no product changes', { paths: [] }, /未检测到/],
  ['metadata-only commits', { subject: 'chore(release): metadata' }, /未检测到/],
]) {
  test(`Rust release rejects ${name} before dispatch`, async () => {
    const result = await runRelease(fixture)
    assert.notEqual(result.status, 0)
    assert.match(result.stderr, message)
    assert.deepEqual(dispatches(result), [])
  })
}

test('mobile-only Rust changes dispatch the App channel', async () => {
  const result = await runRelease({ paths: ['src-tauri/src/mobile/lib.rs'] })
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(
    dispatches(result).map(({ args }) => args[2]),
    ['release-app.yml'],
  )
})

async function generateReleaseNotes({ channel = 'desktop', layout = 'rust' } = {}) {
  const root = await mkdtemp(path.join(tmpdir(), 'pisper-release-notes-'))
  try {
    async function put(relative, contents) {
      const target = path.join(root, relative)
      await mkdir(path.dirname(target), { recursive: true })
      await writeFile(target, contents)
    }
    await mkdir(path.join(root, 'scripts'))
    for (const name of [
      'prepare-release.mjs',
      'prepare-app-release.mjs',
      'release-changes.mjs',
      'release-components.mjs',
      'app-paths.mjs',
    ])
      await copyFile(path.join(repo, 'scripts', name), path.join(root, 'scripts', name))
    await put('package.json', JSON.stringify({ version: '0.6.1' }))
    await put('src-tauri/desktop-package.json', JSON.stringify({ version: '0.6.1' }))
    await put('src-tauri/mobile-package.json', JSON.stringify({ version: '0.1.55' }))
    await put('src-tui/Cargo.toml', '[package]\nname = "pisper-tui"\nversion = "0.6.1"\n')
    await put(layout === 'rust' ? 'runtime-rs/Cargo.toml' : 'runtime/index.mjs', 'fixture')
    const fixturePath = {
      desktop: 'src/features/chat/page.tsx',
      runtime: 'runtime-rs/src/lib.rs',
      tui: 'src-tui/src/main.rs',
      app: 'src-tauri/src/mobile/runtime.rs',
    }[channel]
    const prefix = channel === 'desktop' ? 'v' : `${channel}-v`
    const script = channel === 'app' ? 'prepare-app-release.mjs' : 'prepare-release.mjs'
    const result = spawnSync(
      process.execPath,
      [
        '--import',
        new URL('./fixtures/release-notes-command-fixture.mjs', import.meta.url).href,
        path.join(root, 'scripts', script),
        `${prefix}0.6.1`,
        sourceSha,
        ...(channel === 'app' ? [] : [channel]),
      ],
      {
        cwd: root,
        encoding: 'utf8',
        env: {
          ...process.env,
          GITHUB_REPOSITORY: 'ling-kong-ran/pisper',
          PISPER_RELEASE_NOTES_FIXTURE: JSON.stringify({ path: fixturePath }),
        },
      },
    )
    if (result.error) throw result.error
    assert.equal(result.status, 0, result.stderr)
    const body = await readFile(
      path.join(root, channel === 'app' ? 'release-body-app.md' : 'release-body.md'),
      'utf8',
    )
    const manifest =
      channel === 'tui'
        ? null
        : JSON.parse(
            await readFile(
              path.join(
                root,
                channel === 'app' ? 'docs/latest-app.json' : 'public/release-notes.json',
              ),
              'utf8',
            ),
          )
    return { body, manifest }
  } finally {
    await rm(root, { recursive: true, force: true })
  }
}

test('Rust Desktop release notes explain complete 0.5.x upgrades and preserved data paths', async () => {
  const { body, manifest } = await generateReleaseNotes()
  assert.match(body, /从 0\.5\.x 升级到 Rust 版本/)
  assert.match(body, /0\.5\.80\/0\.5\.81.*Desktop 组件只替换前端/)
  assert.match(body, /releases\/tag\/v0\.6\.1/)
  assert.match(body, /Windows 安装器、macOS DMG 或 Linux AppImage\/DEB/)
  assert.match(body, /~\/\.pisper\/agent/)
  assert.match(body, /PISPER_AGENT_DIR.*保留原设置/)
  assert.match(body, /不要删除.*不需要先卸载/)
  assert.match(body, /尚未完成升级.*完整安装包入口/)
  assert.match(body, /Rust Runtime.*健康检查后.*受管理组件目录.*旧 Node Runtime 和升级提示缓存/)
  assert.match(body, /不删除 Agent 数据、项目工作区或用户下载文件/)
  assert.equal(manifest.notes, body.trim())
})

for (const options of [{ layout: 'node' }, { channel: 'runtime' }, { channel: 'tui' }]) {
  test(`complete Desktop upgrade instructions stay scoped: ${JSON.stringify(options)}`, async () => {
    const { body } = await generateReleaseNotes(options)
    assert.doesNotMatch(body, /从 0\.5\.x 升级到 Rust 版本/)
  })
}

test('App release notes describe embedded Rust and data-preserving installation limits', async () => {
  const { body, manifest } = await generateReleaseNotes({ channel: 'app' })
  for (const notes of [body, manifest.notes]) {
    assert.match(notes, /内置同源 Rust\/Pisper Runtime/)
    assert.doesNotMatch(notes, /Node\/Pisper Runtime|rooted|chroot/)
    assert.match(notes, /相同应用标识、相同签名.*覆盖安装/)
    assert.match(notes, /不要先卸载旧版本/)
    assert.match(notes, /未签名 IPA.*重新签名/)
    assert.match(notes, /不能在 App 内静默覆盖/)
    assert.match(notes, /原应用标识和签名身份/)
  }
  assert.equal(manifest.version, '0.6.1')
  assert.equal(manifest.ipa, 'pisper-ios-unsigned.ipa')
})
