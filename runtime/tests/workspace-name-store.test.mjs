import assert from 'node:assert/strict'
import test from 'node:test'

const STORAGE_KEY = 'pisper-workspace-order'

test('workspace display names persist independently from paths and project order', async (t) => {
  const values = new Map([
    [STORAGE_KEY, JSON.stringify({ state: { order: ['/beta', '/alpha'] }, version: 1 })],
  ])
  let writes = 0
  let writeError = null
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => {
      if (writeError) throw writeError
      values.set(key, value)
      writes += 1
    },
    removeItem: (key) => values.delete(key),
  }
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { localStorage: storage },
  })

  try {
    const { useWorkspaceOrderStore: store } =
      await import('../../src/features/chat/workspace-order-store.ts')

    await t.test('old v1 preferences retain project order without introducing aliases', () => {
      assert.deepEqual(store.getState().order, ['/beta', '/alpha'])
      assert.deepEqual(Object.entries(store.getState().names), [])
      assert.equal(Object.getPrototypeOf(store.getState().names), null)
      assert.equal(writes, 0)
    })

    await t.test('rename, repeat, reload and clear do not reorder projects', async () => {
      store.getState().setWorkspaceName('/alpha', '  客户端  ')
      const renamedState = store.getState()
      const saved = values.get(STORAGE_KEY)
      assert.equal(renamedState.names['/alpha'], '客户端')
      assert.deepEqual(renamedState.order, ['/beta', '/alpha'])
      assert.equal(JSON.parse(saved).version, 1)

      const beforeRepeat = writes
      store.getState().setWorkspaceName('/alpha/', '客户端')
      store.getState().setWorkspaceName('/beta', '  ')
      assert.equal(writes, beforeRepeat)
      assert.equal(store.getState(), renamedState)

      store.setState({ names: Object.create(null), order: [] })
      values.set(STORAGE_KEY, saved)
      await store.persist.rehydrate()
      assert.equal(store.getState().names['/alpha'], '客户端')
      assert.deepEqual(store.getState().order, ['/beta', '/alpha'])

      store.getState().setWorkspaceName('/alpha', ' \t ')
      assert.equal(Object.hasOwn(store.getState().names, '/alpha'), false)
      assert.deepEqual(store.getState().order, ['/beta', '/alpha'])
      await store.persist.rehydrate()
      assert.equal(store.getState().names['/alpha'], undefined)
    })

    await t.test(
      'Windows path aliases and prototype-like project names use safe keys',
      async () => {
        store.getState().setWorkspaceName(' C:\\Work\\Project\\ ', '工作项目')
        store.getState().setWorkspaceName('c:/work/project', '新名称')
        store.getState().setWorkspaceName('__proto__', '原型目录')
        store.getState().setWorkspaceName('toString', '文本目录')
        await store.persist.rehydrate()
        const { names } = store.getState()
        assert.equal(names['c:/work/project'], '新名称')
        assert.equal(names.__proto__, '原型目录')
        assert.equal(names.toString, '文本目录')
        assert.equal(names.constructor, undefined)
        assert.equal(Object.getPrototypeOf(names), null)
        assert.deepEqual(Object.keys(names).sort(), ['__proto__', 'c:/work/project', 'toString'])
        store.getState().setWorkspaceName('__proto__', '')
        assert.equal(store.getState().names.__proto__, undefined)
        assert.equal(Object.getPrototypeOf(store.getState().names), null)
      },
    )

    await t.test('root projects retain names across path aliases and reloads', async () => {
      const originalOrder = store.getState().order
      store.getState().setWorkspaceName('/', '根目录')
      store.getState().setWorkspaceName('///', '系统根目录')
      store.getState().setWorkspaceName(' C:\\ ', '系统盘')
      store.getState().setWorkspaceName('c:/', '本地磁盘')
      store.getState().setWorkspaceName('D:/', '数据盘')
      const saved = values.get(STORAGE_KEY)
      assert.deepEqual(store.getState().order, originalOrder)
      assert.equal(store.getState().names['/'], '系统根目录')
      assert.equal(store.getState().names['c:'], '本地磁盘')
      assert.equal(store.getState().names['d:'], '数据盘')
      assert.equal(store.getState().names['\0'], undefined)

      store.setState({ names: Object.create(null) })
      values.set(STORAGE_KEY, saved)
      await store.persist.rehydrate()
      assert.equal(store.getState().names['/'], '系统根目录')
      assert.equal(store.getState().names['c:'], '本地磁盘')
      assert.equal(store.getState().names['d:'], '数据盘')
      assert.deepEqual(store.getState().order, originalOrder)

      const beforeRepeat = writes
      store.getState().setWorkspaceName('C:', '本地磁盘')
      assert.equal(writes, beforeRepeat)
      store.getState().setWorkspaceName('C:\\', '')
      store.getState().setWorkspaceName('/', '')
      await store.persist.rehydrate()
      assert.equal(store.getState().names['c:'], undefined)
      assert.equal(store.getState().names['/'], undefined)
      assert.equal(store.getState().names['d:'], '数据盘')
    })

    await t.test('invalid names and workspace keys cannot change preferences', () => {
      const current = store.getState()
      const beforeInvalid = writes
      for (const cwd of ['', ' \t ', '\0', '/invalid\0path', null, 12]) {
        assert.throws(() => store.getState().setWorkspaceName(cwd, '名称'), TypeError)
      }
      for (const value of [undefined, null, 12, {}]) {
        assert.throws(() => store.getState().setWorkspaceName('/alpha', value), TypeError)
      }
      assert.throws(() => store.getState().setWorkspaceName('/alpha', '字'.repeat(121)), RangeError)
      assert.equal(store.getState(), current)
      assert.equal(writes, beforeInvalid)
      store.getState().setWorkspaceName('/alpha', '😀'.repeat(120))
      assert.equal(Array.from(store.getState().names['/alpha']).length, 120)
    })

    await t.test(
      'hydration filters malformed preferences and normalizes saved aliases',
      async () => {
        const persisted = JSON.parse(`{
        "state": {
          "order": ["/beta", "/alpha", null, "/beta"],
          "names": {
            " C:\\\\Work\\\\Project\\\\ ": "  工作项目  ",
            "__proto__": "原型目录",
            "toString": "文本目录",
            "": "空目录",
            "\\u0000": "无项目",
            "/invalid\\u0000path": "错误目录",
            "/blank": "  ",
            "/number": 12,
            "/array": [],
            "/object": {},
            "/null": null
          }
        },
        "version": 1
      }`)
        persisted.state.names['/long'] = '字'.repeat(121)
        values.set(STORAGE_KEY, JSON.stringify(persisted))
        await store.persist.rehydrate()
        assert.deepEqual(store.getState().order, ['/beta', '/alpha'])
        assert.deepEqual(Object.entries(store.getState().names).sort(), [
          ['__proto__', '原型目录'],
          ['c:/work/project', '工作项目'],
          ['toString', '文本目录'],
        ])
        assert.equal(Object.getPrototypeOf(store.getState().names), null)
        for (const names of [null, [], 12, 'not a map']) {
          values.set(
            STORAGE_KEY,
            JSON.stringify({ state: { order: ['/alpha'], names }, version: 1 }),
          )
          await store.persist.rehydrate()
          assert.deepEqual(Object.entries(store.getState().names), [])
          assert.deepEqual(store.getState().order, ['/alpha'])
        }
      },
    )

    await t.test(
      'failed persistence restores the previous name and reports the original error',
      () => {
        store.getState().setWorkspaceName('/alpha', '已保存')
        const original = store.getState()
        const saved = values.get(STORAGE_KEY)
        writeError = new Error('Storage write failed')
        assert.throws(
          () => store.getState().setWorkspaceName('/alpha', '未保存'),
          (error) => {
            assert.equal(error, writeError)
            return true
          },
        )
        assert.equal(store.getState().names, original.names)
        assert.equal(store.getState().order, original.order)
        assert.equal(store.getState().names['/alpha'], '已保存')
        assert.equal(values.get(STORAGE_KEY), saved)
        writeError = null
        store.getState().setWorkspaceName('/alpha', '重试已保存')
        assert.equal(store.getState().names['/alpha'], '重试已保存')
        assert.equal(JSON.parse(values.get(STORAGE_KEY)).state.names['/alpha'], '重试已保存')
      },
    )
  } finally {
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else Reflect.deleteProperty(globalThis, 'window')
  }
})
