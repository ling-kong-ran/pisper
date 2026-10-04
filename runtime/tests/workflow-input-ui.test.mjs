import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { setImmediate } from 'node:timers/promises'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import * as inputContract from '../../shared/workflow-inputs.mjs'
import * as graph from '../../shared/workflow-graph.mjs'
import * as imageNodes from '../../shared/workflow-image-nodes.mjs'

const paths = [
  'workflow-templates.ts',
  'workflow-inputs.ts',
  'WorkflowContentField.tsx',
  'WorkflowRunDialog.tsx',
  'useWorkflowEditor.ts',
]
const code = new Map(
  await Promise.all(
    paths.map(async (path) => [
      path,
      transformSync(await readFile(`src/features/workflows/${path}`, 'utf8'), {
        loader: path.endsWith('tsx') ? 'tsx' : 'ts',
        format: 'cjs',
        jsx: 'automatic',
      }).code,
    ]),
  ),
)
const t = (key, values) => `${key}${values?.name ? `:${values.name}` : ''}`
const iconModules = new Proxy({}, { get: (_target, name) => name })
const jsx = (type, props) => ({ type, props })

function load(path, modules = {}, globals = {}) {
  const module = { exports: {} }
  runInNewContext(code.get(path), {
    module,
    exports: module.exports,
    AbortController,
    structuredClone,
    crypto: { randomUUID },
    URL,
    Blob,
    require(name) {
      if (name in modules) return modules[name]
      if (name === 'react/jsx-runtime') return { jsx, jsxs: jsx }
      if (name === 'lucide-react') return iconModules
      if (name === '@/app/use-i18n') return { useI18n: () => ({ t }) }
      if (name === '@shared/workflow-inputs.mjs') return inputContract
      if (name === '@shared/workflow-graph.mjs') return graph
      if (name === '@shared/workflow-image-nodes.mjs') return imageNodes
      throw new Error(`Unexpected test dependency ${name}`)
    },
    ...globals,
  })
  return module.exports
}

const templates = load('workflow-templates.ts')
const inputs = load('workflow-inputs.ts')
code.set(
  'ContentInput',
  transformSync(await readFile('src/components/app/ContentInput.tsx', 'utf8'), {
    loader: 'tsx',
    format: 'cjs',
    jsx: 'automatic',
  }).code,
)
const input = (patch = {}) => ({
  id: 'input-task',
  name: 'task',
  label: 'Task',
  description: '',
  type: 'text',
  defaultValue: '',
  required: false,
  ...patch,
})
const media = (id = 'media-one', mimeType = 'image/png') => ({
  id,
  name: 'sample.png',
  mimeType,
  size: 100,
})
function deferred() {
  let resolve, reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

// 只替换 React 提交调度和外部请求；产品 hook、事件处理器和共享校验均真实执行。
function hooks(context) {
  const slots = []
  let cursor = 0,
    effects = [],
    mounted = true,
    writesAfterUnmount = 0
  const memo = (factory, deps) => {
    const index = cursor++
    const slot = slots[index]
    if (!slot || deps.some((item, i) => !Object.is(item, slot.deps[i])))
      slots[index] = { deps, value: factory() }
    return slots[index].value
  }
  const react = {
    useState(initial) {
      const index = cursor++
      slots[index] ??= { value: typeof initial === 'function' ? initial() : initial }
      return [
        slots[index].value,
        (next) => {
          if (!mounted) writesAfterUnmount += 1
          slots[index].value = typeof next === 'function' ? next(slots[index].value) : next
        },
      ]
    },
    useRef(initial) {
      const index = cursor++
      slots[index] ??= { current: initial }
      return slots[index]
    },
    useMemo: memo,
    useCallback: (callback, deps) => memo(() => callback, deps),
    useId: () => memo(() => randomUUID(), []),
    useEffect(callback, deps) {
      const index = cursor++
      const slot = slots[index]
      if (!slot || deps.some((item, i) => !Object.is(item, slot.deps[i])))
        effects.push({ index, deps, callback })
    },
  }
  const unmount = () => {
    if (!mounted) return
    mounted = false
    for (const slot of slots) slot?.cleanup?.()
  }
  context.after(unmount)
  return {
    react,
    unmount,
    writesAfterUnmount: () => writesAfterUnmount,
    render(component, props) {
      assert.equal(mounted, true)
      cursor = 0
      effects = []
      const result = component(props)
      for (const effect of effects) {
        slots[effect.index]?.cleanup?.()
        slots[effect.index] = { deps: effect.deps, cleanup: effect.callback() }
      }
      return result
    },
  }
}

function find(tree, type) {
  if (!tree || typeof tree !== 'object') return null
  if (tree.type === type) return tree
  for (const child of [tree.props?.children].flat(Infinity)) {
    const found = find(child, type)
    if (found) return found
  }
  return null
}

test('template instances isolate nested nodes, input media defaults and generated identifiers', () => {
  const template = {
    ...templates.WORKFLOW_TEMPLATES[0],
    inputs: [input({ type: 'image', defaultValue: media() })],
  }
  const first = templates.templateWorkflow(template)
  const second = templates.templateWorkflow(template)
  first.inputs[0].defaultValue.name = 'changed.png'
  first.nodes[0].notification.title = 'Changed'
  first.nodes[0].requestedToolNames.push('tool')
  assert.equal(second.inputs[0].defaultValue.name, 'sample.png')
  assert.equal(template.inputs[0].defaultValue.name, 'sample.png')
  assert.notEqual(second.nodes[0].notification.title, 'Changed')
  assert.equal(second.nodes[0].requestedToolNames.length, 0)
  assert.notEqual(first.nodes[0].id, second.nodes[0].id)
  assert.notEqual(first.inputs[0].id, second.inputs[0].id)
  for (const builtIn of templates.WORKFLOW_TEMPLATES.filter((item) => item.id !== 'sprite')) {
    assert.deepEqual(
      [...builtIn.inputs.map((field) => field.name)],
      ['task', 'materials', 'constraints'],
    )
    assert.ok(
      builtIn.nodes
        .filter((node) => ['prompt', 'file', 'skill', 'mcp'].includes(node.kind))
        .every((node) => node.prompt.includes('{{inputs.task}}')),
    )
  }
})

test('run input UI uses shared normalization and translates precise boundary failures', () => {
  const definitions = [
    input({ name: 'count', type: 'number', required: true }),
    input({ id: 'flag', name: 'flag', type: 'boolean', required: true }),
  ]
  assert.deepEqual(inputs.workflowRunInputValues(definitions, { count: '0', flag: false }), {
    count: 0,
    flag: false,
  })
  assert.equal(
    inputs.workflowRunInputError(definitions, { count: 'not-number', flag: false }, t),
    'workflows:inputs.invalidValue:Task',
  )
  assert.equal(
    inputs.workflowRunInputError([input({ required: true })], { task: '  ' }, t),
    'workflows:inputs.requiredValue:Task',
  )
  assert.equal(
    inputs.workflowInputDefinitionError([input({ name: '__proto__' })], t),
    'workflows:inputs.invalidName',
  )
  assert.equal(
    inputs.workflowInputDefinitionError([input(), input({ id: 'duplicate' })], t),
    'workflows:inputs.duplicateName:Task',
  )
  assert.equal(
    inputs.workflowRunInputError([input()], { task: '', extra: 'unknown' }, t),
    'workflows:inputs.unknownInput:extra',
  )
  assert.deepEqual(inputs.workflowRunInputValues([], { task: 'one run only' }), {
    task: 'one run only',
  })
  assert.equal(inputs.workflowRunInputValues([input({ type: 'image' })], { task: '' }).task, null)
})

test('shared content picker clears its native value so selecting the same file again produces another event', (context) => {
  const runtime = hooks(context)
  const selected = []
  const component = load('ContentInput', {
    react: runtime.react,
    '@/components/ui/button': { Button: 'Button' },
    '@/components/ui/label': { Label: 'Label' },
    '@/components/ui/textarea': { Textarea: 'Textarea' },
    '@/lib/utils': { cn: (...classes) => classes.filter(Boolean).join(' ') },
  }).ContentInput
  const tree = runtime.render(component, {
    label: 'File',
    attachmentLabel: 'Choose',
    removeLabel: 'Remove',
    onFilesSelected: (files) => selected.push(files),
    accept: 'image',
  })
  const picker = find(tree, 'input')
  assert.equal(picker.props.accept, 'image/png,image/jpeg,image/webp')
  const file = { name: 'same.png' }
  const nativeInput = { files: [file], value: 'same.png' }
  picker.props.onChange({ currentTarget: nativeInput })
  assert.equal(nativeInput.value, '')
  nativeInput.value = 'same.png'
  picker.props.onChange({ currentTarget: nativeInput })
  assert.equal(selected.length, 2)
  assert.equal(selected[0][0], file)
})

function contentFixture(context) {
  const runtime = hooks(context)
  const requests = [],
    changes = [],
    busy = []
  const component = load('WorkflowContentField.tsx', {
    react: runtime.react,
    '@/components/app/ContentInput': { ContentInput: 'ContentInput' },
    './workflow-media-api': {
      workflowMediaApi: {
        upload(file, signal) {
          const pending = deferred()
          requests.push({ ...pending, file, signal })
          return pending.promise
        },
        preview: async () => new Blob(),
      },
    },
  }).WorkflowContentField
  let props = {
    input: input({ type: 'image' }),
    value: null,
    label: 'Image',
    onChange: (value) => changes.push(value),
    onBusyChange: (id, pending) => busy.push([id, pending]),
  }
  return {
    ...runtime,
    requests,
    changes,
    busy,
    render(patch = {}) {
      props = { ...props, ...patch }
      return runtime.render(component, props)
    },
  }
}

test('content upload locks synchronously and uses the newest callback without reverting edited definitions', async (context) => {
  const f = contentFixture(context)
  const view = f.render()
  const file = { name: 'same.png', type: 'image/png', size: 10 }
  view.props.onFilesSelected([file])
  view.props.onFilesSelected([file])
  assert.equal(f.requests.length, 1)
  const latest = []
  f.render({ onChange: (value) => latest.push(value) })
  f.requests[0].resolve(media())
  await setImmediate()
  assert.equal(f.changes.length, 0)
  assert.equal(latest[0].id, 'media-one')
  assert.equal(f.render().props.disabled, false)
  f.render().props.onFilesSelected([file])
  assert.equal(f.requests.length, 2, 'same file can be selected again after completion')
})

test('switching field type cancels the old upload and its late completion cannot clear the newer upload lock', async (context) => {
  const f = contentFixture(context)
  f.render().props.onFilesSelected([{ name: 'old.png', type: 'image/png', size: 10 }])
  f.render({ input: input({ type: 'video' }) })
  assert.equal(f.requests[0].signal.aborted, true)
  f.render().props.onFilesSelected([{ name: 'next.mp4', type: 'video/mp4', size: 20 }])
  assert.equal(f.requests.length, 2)
  f.requests[0].resolve(media('old'))
  await setImmediate()
  assert.equal(f.changes.length, 0)
  assert.equal(f.render().props.disabled, true)
  f.requests[1].resolve(media('next', 'video/mp4'))
  await setImmediate()
  assert.equal(f.changes[0].id, 'next')
  assert.equal(f.render().props.disabled, false)
})

test('unmount cancels field upload and late responses do not call onChange or write local state', async (context) => {
  const f = contentFixture(context)
  f.render().props.onFilesSelected([{ name: 'source.png', type: 'image/png', size: 10 }])
  f.unmount()
  assert.equal(f.requests[0].signal.aborted, true)
  f.requests[0].resolve(media())
  await setImmediate()
  assert.equal(f.changes.length, 0)
  assert.equal(f.writesAfterUnmount(), 0)
})

test('run submission cannot overtake upload status or duplicate within one render', async (context) => {
  const runtime = hooks(context)
  const requests = [],
    pending = deferred()
  let closed = 0
  const components = Object.fromEntries(
    [
      'Button',
      'Dialog',
      'DialogContent',
      'DialogDescription',
      'DialogFooter',
      'DialogHeader',
      'DialogTitle',
      'Input',
      'Label',
      'Switch',
    ].map((name) => [name, name]),
  )
  const component = load('WorkflowRunDialog.tsx', {
    react: runtime.react,
    ...Object.fromEntries(
      ['button', 'dialog', 'input', 'label', 'switch'].map((name) => [
        `@/components/ui/${name}`,
        components,
      ]),
    ),
    './WorkflowContentField': { WorkflowContentField: 'WorkflowContentField' },
    './workflow-inputs': inputs,
    './workflow-templates': templates,
    './workflow-image-api': {
      workflowImageApi: {
        runNode() {
          throw new Error('Not used by this fixture')
        },
      },
    },
  }).WorkflowRunDialog
  const props = {
    workflow: { name: 'Run', nodes: [], inputs: [input({ type: 'image', required: true })] },
    onClose: () => {
      closed += 1
    },
    onRun: (values) => {
      requests.push(values)
      return pending.promise
    },
  }
  const tree = runtime.render(component, props)
  const field = find(tree, 'WorkflowContentField')
  const submit = () => find(tree, 'form').props.onSubmit({ preventDefault() {} })
  field.props.onBusyChange('input-task', true)
  submit()
  assert.equal(requests.length, 0)
  field.props.onChange(media())
  field.props.onBusyChange('input-task', false)
  submit()
  submit()
  assert.equal(requests.length, 1)
  assert.equal(requests[0].task.id, 'media-one')
  runtime.unmount()
  pending.resolve(true)
  await setImmediate()
  assert.equal(closed, 0)
  assert.equal(runtime.writesAfterUnmount(), 0)
})

const emptyCatalog = {
  workflows: [],
  runs: [],
  notificationTargets: { browser: { enabled: false } },
  models: [],
  skills: [],
  cwd: '',
}
function editorFixture(context) {
  const runtime = hooks(context)
  const requests = [],
    notices = []
  const component = load(
    'useWorkflowEditor.ts',
    {
      react: runtime.react,
      '@/lib/api': {
        apiJson(path, options = {}) {
          const pending = deferred()
          requests.push({ ...pending, path, options })
          return pending.promise
        },
      },
      '@/lib/browser-notifications': { getBrowserNotificationPermission: () => 'denied' },
      './useWorkflowCatalog': { EMPTY_WORKFLOWS_DATA: emptyCatalog, workflowErrorMessage: String },
      './workflow-inputs': inputs,
      './workflow-image-api': {
        workflowImageApi: {
          runNode() {
            throw new Error('Not used by this fixture')
          },
        },
      },
      './workflow-templates': templates,
    },
    { window: { addEventListener() {}, removeEventListener() {}, setInterval, clearInterval } },
  ).useWorkflowEditor
  const props = {
    workflowId: 'new',
    templateId: null,
    notify: (...args) => notices.push(args),
    onCreated() {},
  }
  return {
    ...runtime,
    requests,
    notices,
    render: (patch = {}) => {
      Object.assign(props, patch)
      return runtime.render(component, props)
    },
  }
}

test('editor load failure remains actionable and retry restores the blank canvas draft', async (context) => {
  const f = editorFixture(context)
  f.render()
  f.requests[0].reject(new Error('workflow catalog unavailable'))
  await setImmediate()
  let editor = f.render()
  assert.equal(editor.loading, false)
  assert.equal(editor.draft, null)
  assert.match(editor.error, /workflow catalog unavailable/)

  editor.retryLoad()
  editor = f.render()
  assert.equal(editor.loading, true)
  assert.equal(editor.error, '')
  assert.equal(f.requests.length, 2)
  f.requests[1].resolve(emptyCatalog)
  await setImmediate()
  editor = f.render()
  assert.equal(editor.loading, false)
  assert.equal(editor.error, '')
  assert.equal(editor.draft.nodes.length, 2)
})

test('switching ordinary workflows aborts the previous load and ignores its late result', async (context) => {
  const f = editorFixture(context)
  f.render({ workflowId: 'first' })
  f.render({ workflowId: 'second' })
  assert.equal(f.requests[0].options.signal.aborted, true)
  const first = { ...templates.blankWorkflow(), id: 'first', name: 'First' }
  const second = { ...templates.blankWorkflow(), id: 'second', name: 'Second' }
  f.requests[1].resolve({ ...emptyCatalog, workflows: [first, second] })
  await setImmediate()
  assert.equal(f.render().draft.id, 'second')
  f.requests[0].resolve({ ...emptyCatalog, workflows: [first] })
  await setImmediate()
  assert.equal(f.render().draft.id, 'second')
})

test('editor blocks save and run during default-media upload, serializes saves and keeps newer draft edits', async (context) => {
  const f = editorFixture(context)
  f.render()
  f.requests[0].resolve(emptyCatalog)
  await setImmediate()
  let editor = f.render()
  editor.onInputUploadBusy('image', true)
  assert.equal(await editor.saveWorkflow(), null)
  assert.equal(await editor.runWorkflow({ task: 'run' }), false)
  assert.equal(f.requests.length, 1)
  editor.onInputUploadBusy('image', false)
  editor.updateDraft({ name: 'submitted' })
  editor = f.render()
  const saving = editor.saveWorkflow()
  assert.equal(await editor.saveWorkflow(), null)
  assert.equal(f.requests.length, 2)
  editor.updateDraft({ name: 'newer edit' })
  const saved = { ...JSON.parse(f.requests[1].options.body), id: 'saved', revision: 2 }
  f.requests[1].resolve({ workflow: saved, state: { ...emptyCatalog, workflows: [saved] } })
  await saving
  editor = f.render()
  assert.equal(editor.draft.name, 'newer edit')
  assert.equal(editor.draft.id, 'saved')
})

test('leaving the editor during save aborts its request and prevents a deferred run from starting', async (context) => {
  const f = editorFixture(context)
  f.render()
  f.requests[0].resolve(emptyCatalog)
  await setImmediate()
  const editor = f.render()
  const running = editor.runWorkflow({ task: 'one task' })
  f.unmount()
  assert.equal(f.requests[1].options.signal.aborted, true)
  f.requests[1].resolve({ workflow: { ...editor.draft, id: 'saved' }, state: emptyCatalog })
  assert.equal(await running, false)
  assert.equal(f.requests.length, 2)
  assert.equal(f.writesAfterUnmount(), 0)
})

test('first template run changes route only after save and run have completed', async (context) => {
  const f = editorFixture(context)
  const navigations = []
  f.render({
    onCreated(id, quiet) {
      navigations.push({ id, quiet })
      f.render({ workflowId: id })
    },
  })
  f.requests[0].resolve(emptyCatalog)
  await setImmediate()
  const editor = f.render()
  const running = editor.runWorkflow({ task: 'First run' })
  const saved = { ...editor.draft, id: 'saved' }
  const state = { ...emptyCatalog, workflows: [saved] }
  f.requests[1].resolve({ workflow: saved, state })
  await setImmediate()
  assert.deepEqual(navigations, [])
  assert.equal(f.requests[2].path, '/api/workflows/saved/run')
  assert.equal(f.requests[2].options.signal.aborted, false)
  f.requests[2].resolve({})
  await setImmediate()
  assert.deepEqual(navigations, [])
  f.requests[3].resolve(state)
  assert.equal(await running, true)
  assert.deepEqual(navigations, [{ id: 'saved', quiet: true }])
})

test('an upload that begins while a run is saving prevents generation with incomplete defaults', async (context) => {
  const f = editorFixture(context)
  f.render()
  f.requests[0].resolve(emptyCatalog)
  await setImmediate()
  const editor = f.render()
  const running = editor.runWorkflow({ task: 'one task' })
  editor.onInputUploadBusy('image', true)
  f.requests[1].resolve({ workflow: { ...editor.draft, id: 'saved' }, state: emptyCatalog })
  assert.equal(await running, false)
  assert.equal(f.requests.length, 2)
})

test('stop requests are serialized and route exit cancels the request without a follow-up refresh', async (context) => {
  const f = editorFixture(context)
  f.render({ workflowId: 'saved' })
  f.requests[0].resolve({
    ...emptyCatalog,
    workflows: [{ ...templates.blankWorkflow(), id: 'saved' }],
    runs: [
      {
        id: 'run',
        workflowId: 'saved',
        status: 'waiting_approval',
        startedAt: '2026-09-27T00:00:00Z',
      },
    ],
  })
  await setImmediate()
  const editor = f.render()
  const stopping = editor.stopWorkflow()
  await editor.stopWorkflow()
  assert.equal(f.requests.length, 2)
  f.unmount()
  assert.equal(f.requests[1].options.signal.aborted, true)
  f.requests[1].resolve({})
  await stopping
  assert.equal(f.requests.length, 2)
  assert.equal(f.writesAfterUnmount(), 0)
})
