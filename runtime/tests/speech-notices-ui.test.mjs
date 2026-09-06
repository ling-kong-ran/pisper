import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from '@ts-morph/common/dist/typescript.js'

const notices = JSON.parse(await readFile('shared/speech-resource-notices.json', 'utf8'))
const code = ts.transpileModule(
  await readFile('src/features/chat/SpeechResourceNoticesDialog.tsx', 'utf8'),
  {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2022,
      jsx: ts.JsxEmit.ReactJSX,
    },
  },
).outputText

function load() {
  const exported = {}
  const downloads = []
  const blobs = []
  const timers = []
  const revoked = []
  const primitives = new Proxy({}, { get: (_, name) => name })
  const jsx = (type, props) => ({ type, props })
  runInNewContext(code, {
    exports: exported,
    Blob,
    URL: {
      createObjectURL(blob) {
        blobs.push(blob)
        return 'blob:notice-fixture'
      },
      revokeObjectURL(url) {
        revoked.push(url)
      },
    },
    document: {
      createElement(tag) {
        assert.equal(tag, 'a')
        return {
          click() {
            downloads.push({ href: this.href, download: this.download })
          },
        }
      },
    },
    window: { setTimeout: (callback) => timers.push(callback) },
    require(name) {
      if (name === 'react/jsx-runtime') return { jsx, jsxs: jsx }
      if (name === './speech-resource-notices') return { speechResourceNotices: notices }
      if (name === '@/app/use-i18n') return { useI18n: () => ({ t: (key) => key }) }
      if (name === 'lucide-react' || name.startsWith('@/components/ui/')) return primitives
      throw new Error(`Unexpected notice dependency: ${name}`)
    },
  })
  return { render: exported.SpeechResourceNoticesDialog, downloads, blobs, timers, revoked }
}

function find(node, type) {
  if (!node || typeof node !== 'object') return null
  if (node.type === type) return node
  for (const child of [node.props?.children].flat(Infinity)) {
    const result = find(child, type)
    if (result) return result
  }
  return null
}

function props(overrides = {}) {
  return {
    modelId: 'vits-melo-tts-zh_en',
    modelName: 'MeloTTS Chinese + English FP32',
    returnFocus: { current: null },
    onClose() {},
    ...overrides,
  }
}

test('controlled notices restore a connected trigger and preserve the outer Escape handler', () => {
  const { render } = load()
  let focused = 0
  let prevented = 0
  let stopped = 0
  let closed = 0
  const trigger = { isConnected: true, focus: () => focused++ }
  const root = render(props({ returnFocus: { current: trigger }, onClose: () => closed++ }))
  const content = find(root, 'DialogContent')
  content.props.onCloseAutoFocus({ preventDefault: () => prevented++ })
  assert.equal(prevented, 1)
  assert.equal(focused, 1)
  trigger.isConnected = false
  content.props.onCloseAutoFocus({ preventDefault: () => prevented++ })
  assert.equal(focused, 1)
  content.props.onEscapeKeyDown({ stopPropagation: () => stopped++ })
  assert.equal(stopped, 1)
  root.props.onOpenChange(true)
  assert.equal(closed, 0)
  root.props.onOpenChange(false)
  assert.equal(closed, 1)
  assert.ok(content.props.className.split(' ').includes('grid-cols-1'))
})

test('offline export retains pending provenance and only the selected model licenses', async () => {
  const fixture = load()
  const before = JSON.stringify(notices)
  find(fixture.render(props()), 'Button').props.onClick()
  assert.equal(fixture.downloads.length, 1)
  assert.equal(fixture.downloads[0].download, 'vits-melo-tts-zh_en-notices.json')
  assert.equal(fixture.blobs[0].type, 'application/json;charset=utf-8')
  const data = JSON.parse(await fixture.blobs[0].text())
  const model = notices.models.find((item) => item.id === props().modelId)
  const licenseIds = new Set(model.groups.flatMap((group) => group.licenseIds))
  assert.deepEqual(data.models, [model])
  assert.deepEqual(
    data.licenses,
    notices.licenses.filter((item) => licenseIds.has(item.id)),
  )
  assert.ok(data.models[0].groups.some((group) => group.status === 'pending'))
  assert.equal(JSON.stringify(notices), before)
  assert.deepEqual(fixture.revoked, [])
  assert.equal(fixture.timers.length, 1)
  fixture.timers[0]()
  assert.deepEqual(fixture.revoked, ['blob:notice-fixture'])
})

test('unknown notice identities render nothing and cannot export another model', () => {
  const fixture = load()
  assert.equal(fixture.render(props({ modelId: 'unknown' })), null)
  assert.deepEqual(fixture.downloads, [])
})
