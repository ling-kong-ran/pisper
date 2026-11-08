// Windows 打包前放好 Tauri 的 NSIS 工具链：文件齐全且插件摘要对时不下载；
// 下载按 Tauri 自己的 SHA1 校验；固定的地址与摘要跟锁定的 @tauri-apps/cli 版本走。
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import {
  NSIS_REQUIRED_FILES,
  appImageTools,
  stageTauriAppImageTools,
  NSIS_TAURI_UTILS,
  TAURI_CLI_VERSION,
  stageTauriNsis,
  tauriToolsDirectory,
} from '../../scripts/stage-tauri-tools.mjs'

const sha1 = (value) => createHash('sha1').update(value).digest('hex')
const nsis = {
  url: 'https://github.com/tauri-apps/binary-releases/releases/download/nsis-x/nsis-x.zip',
  sha1: sha1('zip'),
  root: 'nsis-x',
}
const plugin = { ...NSIS_TAURI_UTILS, sha1: sha1('dll') }
// 假的“解包”：按 Tauri 检查的文件把目录铺出来（插件除外，由脚本单独放）。
const extractZip = async (_archive, cwd) => {
  for (const file of NSIS_REQUIRED_FILES) {
    if (file === NSIS_TAURI_UTILS.path) continue
    await mkdir(dirname(join(cwd, 'nsis-x', file)), { recursive: true })
    await writeFile(join(cwd, 'nsis-x', file), 'x')
  }
}

test('the NSIS toolset is staged once from verified downloads', async (t) => {
  const toolsDir = await mkdtemp(join(tmpdir(), 'pisper-tauri-tools-'))
  t.after(() => rm(toolsDir, { recursive: true, force: true }))
  const urls = []
  const fetchImpl = async (url) => {
    urls.push(url)
    return new Response(url.endsWith('.zip') ? 'zip' : 'dll')
  }
  const first = await stageTauriNsis({ toolsDir, nsis, plugin, fetchImpl, extractZip })
  assert.equal(first.staged, true)
  assert.deepEqual(urls.sort(), [NSIS_TAURI_UTILS.url, nsis.url].sort())
  assert.equal(await readFile(join(toolsDir, 'NSIS', NSIS_TAURI_UTILS.path), 'utf8'), 'dll')
  // 临时解包目录清掉，只留下 NSIS。
  assert.deepEqual(await readdir(toolsDir), ['NSIS'])
  urls.length = 0
  assert.equal(
    (await stageTauriNsis({ toolsDir, nsis, plugin, fetchImpl, extractZip })).staged,
    false,
  )
  assert.deepEqual(urls, [])
})

test('a tampered download is rejected before anything is written', async (t) => {
  const toolsDir = await mkdtemp(join(tmpdir(), 'pisper-tauri-tools-'))
  t.after(() => rm(toolsDir, { recursive: true, force: true }))
  await assert.rejects(
    stageTauriNsis({
      toolsDir,
      nsis,
      plugin,
      retryDelayMs: 1,
      extractZip,
      fetchImpl: async () => new Response('tampered'),
    }),
    /SHA1 mismatch/,
  )
  assert.deepEqual(await readdir(toolsDir), [])
})

test('pinned NSIS assets follow the locked Tauri CLI and run before the Windows and Linux bundles', async () => {
  const lock = JSON.parse(await readFile('package-lock.json', 'utf8'))
  // 升级 @tauri-apps/cli 时，按新 tauri-bundler 的 nsis/mod.rs 与 linuxdeploy.rs 更新 stage-tauri-tools.mjs。
  assert.equal(lock.packages['node_modules/@tauri-apps/cli'].version, TAURI_CLI_VERSION)
  assert.equal(
    tauriToolsDirectory({ LOCALAPPDATA: 'C:\\Users\\me\\AppData\\Local' }, 'win32'),
    'C:\\Users\\me\\AppData\\Local\\tauri',
  )
  assert.equal(tauriToolsDirectory({ HOME: '/home/runner' }, 'linux'), '/home/runner/.cache/tauri')
  const packaging = await readFile('scripts/package-tauri-release.mjs', 'utf8')
  assert.match(
    packaging,
    /if \(process\.platform === 'win32' \|\| process\.platform === 'linux'\)\s+await run\(process\.execPath, \[path\.join\(root, 'scripts', 'stage-tauri-tools\.mjs'\)\], env\)\s+[\s\S]*?const buildArgs = \[tauriCli, 'build'/,
  )
})

test('AppImage tools are staged once, executable, and the optional plugin may be missing', async (t) => {
  const toolsDir = await mkdtemp(join(tmpdir(), 'pisper-appimage-tools-'))
  t.after(() => rm(toolsDir, { recursive: true, force: true }))
  const urls = []
  const fetchImpl = async (url) => {
    urls.push(url)
    if (url.includes('linuxdeploy-plugin-appimage')) return new Response('gone', { status: 404 })
    return new Response(`#!/bin/sh\n# ${url}\n`)
  }
  const first = await stageTauriAppImageTools({ toolsDir, fetchImpl, retryDelayMs: 1 })
  assert.deepEqual(
    first.staged,
    appImageTools()
      .filter((tool) => !tool.optional)
      .map((tool) => tool.file),
  )
  assert.equal(urls.length, appImageTools().length)
  const { mode } = await import('node:fs/promises').then(({ stat }) =>
    stat(join(toolsDir, 'linuxdeploy-x86_64.AppImage')),
  )
  assert.equal(mode & 0o111, 0o111)
  assert.ok(!(await readdir(toolsDir)).some((name) => name.endsWith('.partial')))
  urls.length = 0
  assert.deepEqual(
    (await stageTauriAppImageTools({ toolsDir, fetchImpl, retryDelayMs: 1 })).staged,
    [],
  )
  // 只剩可选插件没有：再试一次，仍然下不到也不报错。
  assert.equal(urls.length, 1)
})
