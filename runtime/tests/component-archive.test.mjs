import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import {
  chmod,
  copyFile,
  cp,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  symlink,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { promisify } from 'node:util'
import { runInNewContext } from 'node:vm'
import { parse } from '@babel/parser'
import test from 'node:test'
import { t as listTar, x as extractTar } from 'tar'

const run = promisify(execFile)
const suffix = process.platform === 'win32' ? '.exe' : ''

async function fixture(t) {
  const root = await mkdtemp(join(await realpath(tmpdir()), 'pisper-component-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const runtime = join(root, 'release', 'sea', 'runtime')
  await Promise.all([
    mkdir(join(root, 'scripts'), { recursive: true }),
    mkdir(join(root, 'src-tauri'), { recursive: true }),
    mkdir(join(root, 'src-tui'), { recursive: true }),
    mkdir(runtime, { recursive: true }),
  ])
  for (const name of ['archive-component-release.mjs', 'release-components.mjs']) {
    await copyFile(join('scripts', name), join(root, 'scripts', name))
  }
  await symlink(resolve('node_modules'), join(root, 'node_modules'), 'junction')
  await writeFile(join(root, 'package.json'), '{"version":"1.2.3"}')
  await writeFile(join(root, 'src-tauri', 'desktop-package.json'), '{"version":"1.2.3"}')
  await writeFile(join(root, 'src-tui', 'Cargo.toml'), '[package]\nversion = "1.2.3"\n')
  await writeFile(join(root, 'release', 'sea', `pisper-sidecar${suffix}`), 'sidecar')
  await writeFile(join(root, 'release', 'sea', 'runtime-size-manifest.json'), '{}')
  await writeFile(join(runtime, 'package.json'), '{}')
  return { root, runtime }
}

const supportedTypes = new Set(['File', 'Directory'])

async function entries(archive) {
  const result = []
  await listTar({
    file: archive,
    strict: true,
    onReadEntry(entry) {
      result.push({ path: entry.path, type: entry.type })
    },
  })
  return result
}

test(
  'runtime component archives materialize safe links and preserve npm bin execution',
  { skip: process.platform === 'win32' && 'Windows npm does not stage file symlinks' },
  async (t) => {
    const { root, runtime } = await fixture(t)
    const packageRoot = join(runtime, 'node_modules', 'example')
    await mkdir(join(runtime, 'node_modules', '.bin'), { recursive: true })
    await mkdir(join(packageRoot, 'bin'), { recursive: true })
    await writeFile(join(packageRoot, 'bin', 'value.cjs'), 'module.exports = "ok"\n')
    await writeFile(
      join(packageRoot, 'bin', 'cli.cjs'),
      '#!/usr/bin/env node\nconsole.log(JSON.stringify([require("./value.cjs"), ...process.argv.slice(2)]))\n',
    )
    await chmod(join(packageRoot, 'bin', 'cli.cjs'), 0o755)
    await symlink('../example/bin/cli.cjs', join(runtime, 'node_modules', '.bin', 'example'))
    await writeFile(join(packageRoot, 'lib.1.dylib'), 'library')
    await symlink('lib.1.dylib', join(packageRoot, 'lib.dylib'))

    await run(
      process.execPath,
      [join(root, 'scripts', 'archive-component-release.mjs'), 'runtime'],
      {
        cwd: root,
      },
    )

    const output = join(root, 'release', 'component-artifacts')
    const archives = (await readdir(output)).filter((name) => name.endsWith('.tar.gz'))
    assert.equal(archives.length, 2)
    for (const name of archives) {
      const archive = join(output, name)
      const archiveEntries = await entries(archive)
      assert.ok(archiveEntries.length > 0)
      assert.deepEqual(
        archiveEntries.filter((entry) => !supportedTypes.has(entry.type)),
        [],
      )
      const destination = join(root, 'extracted', name)
      await mkdir(destination, { recursive: true })
      await extractTar({ file: archive, cwd: destination, strip: 1, strict: true })
      const payload = join(destination, 'sidecar-runtime')
      const launcher = join(payload, 'node_modules', '.bin', 'example')
      assert.ok((await lstat(launcher)).isFile())
      const result = await run(launcher, ['argument with spaces', "quote'arg"])
      assert.deepEqual(JSON.parse(result.stdout), ['ok', 'argument with spaces', "quote'arg"])
      assert.equal(
        await readFile(join(payload, 'node_modules', 'example', 'lib.dylib'), 'utf8'),
        'library',
      )
    }
  },
)

test(
  'TUI staging preserves relative npm links before secure component archiving',
  { skip: process.platform === 'win32' && 'Windows npm does not stage file symlinks' },
  async (t) => {
    const { root, runtime } = await fixture(t)
    await mkdir(join(runtime, 'node_modules', '.bin'), { recursive: true })
    await mkdir(join(runtime, 'node_modules', 'example'), { recursive: true })
    await writeFile(join(runtime, 'node_modules', 'example', 'cli.cjs'), 'console.log("ok")\n')
    await symlink('../example/cli.cjs', join(runtime, 'node_modules', '.bin', 'example'))
    const source = await readFile('scripts/package-tui.mjs', 'utf8')
    let copyCall
    const visit = (node) => {
      if (!node || typeof node !== 'object') return
      if (
        node.type === 'CallExpression' &&
        node.callee?.name === 'cp' &&
        node.arguments[0]?.name === 'runtimeSource'
      )
        copyCall = node
      for (const value of Object.values(node)) {
        if (Array.isArray(value)) value.forEach(visit)
        else if (value && typeof value === 'object') visit(value)
      }
    }
    visit(parse(source, { sourceType: 'module' }))
    assert.ok(copyCall)
    const stage = join(root, 'tui-stage')
    // 执行生产 cp 表达式，避免 fixture 自己使用正确选项而掩盖打包回归。
    await runInNewContext(source.slice(copyCall.start, copyCall.end), {
      cp,
      join,
      stage,
      runtimeSource: runtime,
    })
    await writeFile(join(stage, `pisper${suffix}`), 'tui')
    await run(process.execPath, [join(root, 'scripts', 'archive-component-release.mjs'), 'tui'], {
      cwd: root,
      env: { ...process.env, PISPER_TUI_STAGE_DIR: stage },
    })
    const output = join(root, 'release', 'component-artifacts')
    for (const name of (await readdir(output)).filter((name) => name.endsWith('.tar.gz'))) {
      assert.deepEqual(
        (await entries(join(output, name))).filter((entry) => !supportedTypes.has(entry.type)),
        [],
      )
    }
  },
)

test(
  'runtime component packaging rejects links outside its source root',
  { skip: process.platform === 'win32' && 'Windows file symlinks require elevated privileges' },
  async (t) => {
    const { root, runtime } = await fixture(t)
    const outside = join(root, 'outside.txt')
    await writeFile(outside, 'private')
    await symlink(outside, join(runtime, 'escape'))
    await assert.rejects(
      run(process.execPath, [join(root, 'scripts', 'archive-component-release.mjs'), 'runtime'], {
        cwd: root,
      }),
      /escapes its source root/,
    )
  },
)
