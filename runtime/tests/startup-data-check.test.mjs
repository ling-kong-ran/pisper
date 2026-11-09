import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { parseRuntimeStartupStatus } from '../../src/app/runtime-startup-api.ts'
import { createPisperRuntime } from '../app-runtime.mjs'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import {
  checkDataFiles,
  describeStartupError,
  setAsideDataFile,
} from '../services/startup-data-check.mjs'

async function tempDir(t) {
  const dir = await mkdtemp(join(tmpdir(), 'pisper-data-check-'))
  t.after(() => rm(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))
  return dir
}

test('checkDataFiles sets aside unreadable JSON and leaves everything else', async (t) => {
  const dir = await tempDir(t)
  await writeFile(join(dir, 'good.json'), '{"a":1}')
  await writeFile(join(dir, 'list.json'), '[1]')
  await writeFile(join(dir, 'truncated.json'), '{"a":')
  await writeFile(join(dir, 'empty.json'), '  \n')
  await writeFile(join(dir, 'scalar.json'), 'null')
  await writeFile(join(dir, 'notes.txt'), '{"a":')
  await mkdir(join(dir, 'nested'))
  await writeFile(join(dir, 'nested', 'broken.json'), '{"a":')

  const moved = await checkDataFiles(dir, { confirmDelayMs: 0 })

  assert.deepEqual(moved.map((item) => [item.file, item.reason]).sort(), [
    ['empty.json', 'empty'],
    ['scalar.json', 'wrong_shape'],
    ['truncated.json', 'unparsable'],
  ])
  const names = await readdir(dir)
  assert.ok(
    names.includes('good.json') && names.includes('list.json') && names.includes('notes.txt'),
  )
  assert.ok(!names.includes('truncated.json'))
  const kept = names.find((name) => name.startsWith('truncated.json.corrupt-'))
  assert.equal(await readFile(join(dir, kept), 'utf8'), '{"a":')
  assert.equal(await readFile(join(dir, 'nested', 'broken.json'), 'utf8'), '{"a":')
  // 留底文件不以 .json 结尾：下次启动不会再处理一遍。
  assert.deepEqual(await checkDataFiles(dir, { confirmDelayMs: 0 }), [])
})

test(
  'checkDataFiles strips a BOM in place and keeps the file mode',
  { skip: process.platform === 'win32' },
  async (t) => {
    const dir = await tempDir(t)
    const path = join(dir, 'auth.json')
    await writeFile(path, '﻿{"key":"value"}', { mode: 0o600 })
    assert.deepEqual(await checkDataFiles(dir, { confirmDelayMs: 0 }), [])
    assert.equal(await readFile(path, 'utf8'), '{"key":"value"}')
    assert.equal((await stat(path)).mode & 0o777, 0o600)
  },
)

test('a file another process finishes writing during the check is not moved', async (t) => {
  const dir = await tempDir(t)
  const path = join(dir, 'shared.json')
  await writeFile(path, '{"half":')
  const check = checkDataFiles(dir, { confirmDelayMs: 60 })
  await new Promise((resume) => setTimeout(resume, 20))
  await writeFile(path, '{"half":false}')
  assert.deepEqual(await check, [])
  assert.equal(await readFile(path, 'utf8'), '{"half":false}')
})

test('a missing data directory is not an error', async (t) => {
  const dir = await tempDir(t)
  assert.deepEqual(await checkDataFiles(join(dir, 'nope')), [])
  assert.equal(await setAsideDataFile(join(dir, 'nope.json')), '')
})

test('startup errors shown to people carry no personal paths', () => {
  const error = Object.assign(
    new Error("EACCES: open '/home/me/.pisper/agent/x.json' and /home/me/y"),
    {
      code: 'EACCES',
    },
  )
  const description = describeStartupError(error, {
    dataDir: '/home/me/.pisper/agent',
    home: '/home/me',
  })
  assert.equal(description.code, 'EACCES')
  assert.equal(description.message, "EACCES: open '<data>/x.json' and ~/y")
  assert.equal(describeStartupError('boom', { home: '' }).code, 'unknown')
  assert.ok(describeStartupError(new Error('x'.repeat(2000))).message.length <= 300)
})

test(
  'READY does not wait for remote access or for initialization',
  { timeout: 15_000 },
  async (t) => {
    const dir = await tempDir(t)
    await writeFile(join(dir, 'package.json'), '{"version":"0.0.0"}')
    let releaseFirewall
    const firewallGate = new Promise((resume) => (releaseFirewall = resume))
    let rejectModules
    const modulesGate = new Promise((_resolve, reject) => (rejectModules = reject))
    const firewall = { reconcile: () => firewallGate, status: () => ({}) }
    // 防火墙检查和初始化都卡住：createPisperRuntime 返回后 sidecar 才写 READY，所以它必须照样返回。
    const app = await createPisperRuntime({
      root: dir,
      dataDir: dir,
      runtimeCwd: dir,
      production: true,
      port: 0,
      deferRuntimeInitialization: true,
      runtimeCapabilities: { profile: 'desktop', features: { remoteAccess: true } },
      remote: { enabled: true, port: 0, host: '127.0.0.1', firewallService: firewall },
      runtimeModuleLoader: () => modulesGate,
      apiHandlerModuleLoader: () => modulesGate,
    })
    t.after(async () => {
      releaseFirewall()
      rejectModules(new Error('test finished'))
      await app.close()
    })
    assert.equal(app.remoteControl.status().enabled, true)
    const status = await fetch(`${app.url}/api/startup-status`).then((response) => response.json())
    assert.equal(status.state, 'pending')
  },
)

test('a failed start is reported with a reason at /api/startup-status', async (t) => {
  const dir = await tempDir(t)
  const app = await createPisperRuntime({
    root: dir,
    dataDir: dir,
    runtimeCwd: dir,
    production: true,
    port: 0,
    deferRuntimeInitialization: true,
    runtimeModuleLoader: async () => {
      throw Object.assign(new Error(`cannot read ${dir}/x`), { code: 'EACCES' })
    },
  })
  t.after(() => app.close())
  await assert.rejects(app.initialized)
  const status = await fetch(`${app.url}/api/startup-status`).then((response) => response.json())
  assert.equal(status.state, 'failed')
  assert.deepEqual(status.failure, { code: 'EACCES', message: 'cannot read <data>/x' })
})

test('the memory index catch-up keeps what it finished when interrupted', async (t) => {
  const dir = await tempDir(t)
  const path = join(dir, 'memory.sqlite')
  const first = new LocalMemoryRuntime({ path, cwd: dir })
  await first.init()
  for (let index = 0; index < 1200; index += 1) {
    first.remember({
      title: `标题 ${index}`,
      content: `内容 ${index}`,
      spaceId: 'global',
      type: 'project',
    })
  }
  first.requireDb().exec('UPDATE memories SET indexed_at = NULL')
  await first.dispose()

  // 模拟补到一半被杀：第 700 条抛错。
  const interrupted = new LocalMemoryRuntime({ path, cwd: dir })
  const indexFact = interrupted.indexFact.bind(interrupted)
  let calls = 0
  interrupted.indexFact = (id) => {
    calls += 1
    if (calls === 700) throw new Error('killed')
    return indexFact(id)
  }
  await assert.rejects(interrupted.init(), /killed/)
  assert.equal(interrupted.db, null)

  const resumed = new LocalMemoryRuntime({ path, cwd: dir })
  const indexResumed = resumed.indexFact.bind(resumed)
  let resumedCalls = 0
  resumed.indexFact = (id) => {
    resumedCalls += 1
    return indexResumed(id)
  }
  await resumed.init()
  // 在这里关库，不用 t.after：t.after 按注册顺序执行，删临时目录排在前面，
  // Windows 上数据库还开着时 memory.sqlite-wal 删不掉（EBUSY）。
  try {
    assert.equal(resumedCalls, 700, 'the first full batch was kept, only the rest is redone')
    const left = resumed
      .requireDb()
      .prepare('SELECT COUNT(*) AS n FROM memories WHERE indexed_at IS NULL')
    assert.equal(left.get().n, 0)
  } finally {
    await resumed.dispose()
  }
})

test('the page reads the startup status defensively', () => {
  assert.deepEqual(parseRuntimeStartupStatus(null), {
    state: 'pending',
    failure: null,
    setAsideFiles: [],
    failedServices: [],
  })
  assert.deepEqual(
    parseRuntimeStartupStatus({
      state: 'failed',
      failure: { code: 'EACCES', message: 'denied' },
      setAside: [{ file: 'a.json' }, null, { file: 1 }],
      failed: [{ service: 'memory' }, 'x'],
    }),
    {
      state: 'failed',
      failure: { code: 'EACCES', message: 'denied' },
      setAsideFiles: ['a.json'],
      failedServices: ['memory'],
    },
  )
  assert.equal(parseRuntimeStartupStatus({ state: 'weird' }).state, 'pending')
})
