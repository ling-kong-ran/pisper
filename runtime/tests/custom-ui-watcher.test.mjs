// 组件目录监听：强制轮询模式下的变化 / 新增 / 删除、点开头路径忽略、按组件去抖、dispose 后不再回调；
// fs.watch 不可用时退回轮询；服务层按 rev 去重（写入器发过的不重复发）。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { CustomUiService } from '../services/custom-ui-service.mjs'
import { ComponentWatcher, componentIdForPath } from '../services/custom-ui-watcher.mjs'

const delay = (ms) => new Promise((resolveDelay) => setTimeout(resolveDelay, ms))

/** 轮询直到条件成立（每 20ms 查一次），超时给出清楚的失败信息，而不是任意等待。 */
async function until(predicate, message, timeoutMs = 4000) {
  const started = Date.now()
  while (!predicate()) {
    if (Date.now() - started > timeoutMs) assert.fail(message)
    await delay(20)
  }
}

async function root(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-watch-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  return join(directory, 'custom-ui')
}

test('路径归属：点开头的段与不合法 id 都忽略', () => {
  assert.equal(componentIdForPath('demo/component.js'), 'demo')
  assert.equal(componentIdForPath('demo\\lib\\a.js'), 'demo')
  assert.equal(componentIdForPath('demo'), 'demo')
  assert.equal(componentIdForPath('.component-write-abc/component.js'), null)
  assert.equal(componentIdForPath('.component-old-abc'), null)
  assert.equal(componentIdForPath('demo/.component.js.swp'), null)
  assert.equal(componentIdForPath('Demo/x.js'), null)
  assert.equal(componentIdForPath(''), null)
  assert.equal(componentIdForPath(null), null)
})

test('强制轮询：变化、新增、删除都按组件去抖回调一次；点开头目录忽略；dispose 后停止', async (t) => {
  const dir = await root(t)
  await mkdir(join(dir, 'alpha'), { recursive: true })
  await writeFile(join(dir, 'alpha', 'component.js'), 'export default 1')
  const changes = []
  const watcher = new ComponentWatcher({
    root: dir,
    onChange: (id) => changes.push(id),
    forcePoll: true,
    // 去抖长于轮询间隔：跨两次扫描的连续写入也只合并成一次回调。
    pollMs: 40,
    debounceMs: 120,
  })
  t.after(() => watcher.dispose())
  assert.equal(await watcher.start(), 'poll')
  // 基线不产生通知。
  await delay(120)
  assert.deepEqual(changes, [])

  await writeFile(join(dir, 'alpha', 'component.js'), 'export default 22')
  await writeFile(join(dir, 'alpha', 'extra.js'), 'export const x = 1')
  await until(() => changes.includes('alpha'), 'alpha 的变化没有被发现')
  await delay(250)
  assert.deepEqual(changes, ['alpha'])

  await mkdir(join(dir, '.component-write-xyz'), { recursive: true })
  await writeFile(join(dir, '.component-write-xyz', 'component.js'), 'staging')
  await writeFile(join(dir, 'alpha', '.swap'), 'editor temp')
  await delay(300)
  assert.deepEqual(changes, ['alpha'])

  await mkdir(join(dir, 'beta'))
  await writeFile(join(dir, 'beta', 'manifest.json'), '{}')
  await until(() => changes.includes('beta'), '新增的 beta 没有被发现')
  await rm(join(dir, 'alpha'), { recursive: true })
  await until(() => changes.filter((id) => id === 'alpha').length === 2, '删除 alpha 没有通知')

  watcher.dispose()
  const count = changes.length
  await writeFile(join(dir, 'beta', 'manifest.json'), '{"changed":true}')
  await delay(200)
  assert.equal(changes.length, count)
})

test('fs.watch 抛错（不支持递归监听）时退回轮询', async (t) => {
  const dir = await root(t)
  const watcher = new ComponentWatcher({
    root: dir,
    onChange: () => {},
    watch: () => {
      throw Object.assign(new Error('unsupported'), { code: 'ERR_FEATURE_UNAVAILABLE_ON_PLATFORM' })
    },
    pollMs: 1000,
  })
  t.after(() => watcher.dispose())
  assert.equal(await watcher.start(), 'poll')
})

test('fs.watch 事件：按文件名归属组件并去抖，点开头路径忽略，没有文件名时扫描', async (t) => {
  const dir = await root(t)
  let emit = null
  const fake = {
    on() {
      return fake
    },
    unref() {},
    close() {
      fake.closed = true
    },
    closed: false,
  }
  const changes = []
  const watcher = new ComponentWatcher({
    root: dir,
    onChange: (id) => changes.push(id),
    debounceMs: 20,
    watch: (_root, options, listener) => {
      assert.equal(options.recursive, true)
      assert.equal(options.persistent, false)
      emit = listener
      return fake
    },
  })
  assert.equal(await watcher.start(), 'watch')
  emit('change', 'demo/component.js')
  emit('change', 'demo/lib/a.js')
  emit('rename', '.component-write-1/component.js')
  emit('change', 'demo/.hidden')
  await until(() => changes.length === 1, '去抖后没有回调')
  await delay(60)
  assert.deepEqual(changes, ['demo'])
  watcher.dispose()
  assert.equal(fake.closed, true)
})

test('服务层：监听到的变化按 rev 去重；手改文件发 files（带新 rev），手动删除发 deleted', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-watch-service-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const watchers = []
  const service = new CustomUiService({
    dataDir: directory,
    createWatcher: (options) => {
      const watcher = new ComponentWatcher({
        ...options,
        forcePoll: true,
        pollMs: 40,
        debounceMs: 20,
      })
      watchers.push(watcher)
      return watcher
    },
  })
  const events = []
  await service.start({ onChange: (id, detail) => events.push({ id, ...detail }), watch: true })
  t.after(() => service.dispose())
  assert.equal(watchers.length, 1)

  const created = await service.writeComponent(
    {
      mode: 'create',
      id: 'demo',
      manifest: { runtime: 'native', name: 'Demo' },
      files: [{ path: 'component.js', content: 'export default () => null\n' }],
    },
    { source: 'agent' },
  )
  // 写入器自己发了 created；随后轮询看到同一 rev 不再重复发。
  await delay(250)
  assert.deepEqual(
    events.map((event) => event.reason),
    ['created'],
  )

  await writeFile(join(service.root, 'demo', 'component.js'), 'export default () => 1\n')
  await until(() => events.length === 2, '手改文件没有发事件')
  assert.equal(events[1].reason, 'files')
  assert.match(events[1].rev, /^[a-f0-9]{24}$/)
  assert.notEqual(events[1].rev, created.rev)

  await rm(join(service.root, 'demo'), { recursive: true })
  await until(() => events.length === 3, '手动删除没有发事件')
  assert.deepEqual(events[2], { id: 'demo', rev: null, reason: 'deleted' })

  service.dispose()
  assert.equal(watchers[0].mode, 'disposed')
})

test('start 不开 watch 时不建监听；残留的旧导入暂存目录（超过 10 分钟）被清理，.component-old-* 不动', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-cleanup-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const root = join(directory, 'custom-ui')
  await mkdir(join(root, '.component-import-old'), { recursive: true })
  await mkdir(join(root, '.component-old-keep'), { recursive: true })
  let created = 0
  const service = new CustomUiService({
    dataDir: directory,
    now: () => Date.now() + 11 * 60_000,
    createWatcher: () => {
      created += 1
      return { start: async () => {}, dispose() {} }
    },
  })
  await service.start({})
  assert.equal(created, 0)
  assert.deepEqual((await readdir(root)).sort(), ['.component-old-keep'])
  service.dispose()
})
