import assert from 'node:assert/strict'
import test from 'node:test'
import {
  CANVAS_KINDS,
  CANVAS_MAX_DEPTH,
  CANVAS_MAX_NODES,
  CANVAS_TEXT_MAX_LENGTH,
  createDefaultCanvas,
  parseChatCanvas,
  findCanvasNode,
  canvasHasKind,
  isCanvasContainerKind,
  addCanvasNode,
  moveCanvasNode,
  removeCanvasNode,
  updateCanvasNode,
} from '../../src/features/chat/layout/chat-canvas.ts'

const fresh = () => createDefaultCanvas({ composerPosition: 'bottom' })
const lastChild = (root) => root.children.at(-1)

test('canvas kinds and default trees support both composer positions', () => {
  assert.ok(CANVAS_KINDS.includes('grid'))
  assert.equal(isCanvasContainerKind('messages'), false)
  for (const position of ['top', 'bottom']) {
    const root = createDefaultCanvas({ composerPosition: position })
    assert.deepEqual(parseChatCanvas(root), root)
    assert.deepEqual(
      root.children.map((node) => node.kind),
      position === 'top' ? ['header', 'composer', 'messages'] : ['header', 'messages', 'composer'],
    )
    assert.ok(canvasHasKind(root, 'messages'))
    assert.equal(canvasHasKind(root, 'context'), false)
  }
})

test('nested containers and repeated decorative components are accepted and cloned', () => {
  let root = fresh()
  root = addCanvasNode(root, root.id, 'grid')
  const grid = lastChild(root).id
  root = addCanvasNode(root, grid, 'row')
  const row = findCanvasNode(root, grid).children[0].id
  for (const kind of ['text', 'text', 'divider', 'spacer']) root = addCanvasNode(root, row, kind)
  root = moveCanvasNode(root, 'canvas-messages', row, 1)
  const parsed = parseChatCanvas(root)
  assert.notEqual(parsed, root)
  assert.notEqual(parsed.children, root.children)
  assert.equal(findCanvasNode(parsed, row).children[1].kind, 'messages')
  assert.equal(findCanvasNode(parsed, 'missing'), undefined)
})

test('custom UI instances retain component references through editing and JSON round trips', () => {
  const original = fresh()
  const first = addCanvasNode(original, original.id, 'custom-ui', 'my.widget-v2')
  const firstId = lastChild(first).id
  const second = addCanvasNode(first, first.id, 'custom-ui', 'my.widget-v2')
  const secondId = lastChild(second).id
  assert.notEqual(firstId, secondId)
  assert.equal(findCanvasNode(second, firstId).componentId, 'my.widget-v2')
  assert.equal(lastChild(second).componentId, 'my.widget-v2')
  assert.equal(lastChild(second).css, 'height: 240px; flex-shrink: 0;')
  assert.deepEqual(parseChatCanvas(JSON.parse(JSON.stringify(second))), second)
  const moved = moveCanvasNode(second, secondId, second.id, 0)
  assert.equal(moved.children[0].componentId, 'my.widget-v2')
  const changed = updateCanvasNode(moved, firstId, { componentId: 'other_widget' })
  assert.equal(findCanvasNode(changed, firstId).componentId, 'other_widget')
  assert.equal(findCanvasNode(second, firstId).componentId, 'my.widget-v2')
  assert.equal(canvasHasKind(original, 'custom-ui'), false)
  assert.equal(findCanvasNode(removeCanvasNode(changed, firstId), firstId), undefined)
})

test('custom UI component references reject missing, unsafe and misplaced identifiers', () => {
  const root = fresh()
  for (const componentId of [
    undefined,
    null,
    7,
    '',
    'Upper',
    '../escape',
    '/absolute',
    'x/y',
    'x\\y',
    'two words',
    '<widget>',
    'x'.repeat(65),
  ]) {
    assert.throws(() => addCanvasNode(root, root.id, 'custom-ui', componentId), {
      code: 'invalid_canvas',
    })
  }
  assert.doesNotThrow(() => addCanvasNode(root, root.id, 'custom-ui', 'x'.repeat(64)))
  for (const kind of ['text', 'column', 'spacer']) {
    assert.throws(() => addCanvasNode(root, root.id, kind, 'widget'), { code: 'invalid_canvas' })
  }
  for (const componentId of ['widget', undefined]) {
    const misplaced = structuredClone(root)
    misplaced.children[0].componentId = componentId
    assert.throws(() => parseChatCanvas(misplaced), { code: 'invalid_canvas' })
  }
  assert.throws(() => updateCanvasNode(root, 'canvas-header', { componentId: 'widget' }), {
    code: 'invalid_canvas',
  })
  const custom = addCanvasNode(root, root.id, 'custom-ui', 'widget')
  assert.throws(() => updateCanvasNode(custom, lastChild(custom).id, { componentId: '../bad' }), {
    code: 'invalid_canvas',
  })
  const container = structuredClone(custom)
  lastChild(container).children = []
  assert.throws(() => parseChatCanvas(container), { code: 'invalid_canvas' })
  let invoked = false
  Object.defineProperty(lastChild(custom), 'componentId', {
    get() {
      invoked = true
      return 'widget'
    },
  })
  assert.throws(() => parseChatCanvas(custom), { code: 'invalid_canvas' })
  assert.equal(invoked, false)
})

test('tree edits are immutable and core functional blocks cannot be duplicated or removed', () => {
  const root = fresh()
  const before = structuredClone(root)
  for (const kind of ['messages', 'composer', 'header'])
    assert.throws(() => addCanvasNode(root, root.id, kind), { code: 'invalid_canvas' })
  for (const id of ['canvas-messages', 'canvas-composer', 'canvas-root'])
    assert.throws(() => removeCanvasNode(root, id), { code: 'invalid_canvas' })
  const withoutHeader = removeCanvasNode(root, 'canvas-header')
  assert.equal(canvasHasKind(withoutHeader, 'header'), false)
  const styled = updateCanvasNode(root, 'canvas-messages', {
    css: 'padding: 12px; color: var(--text);',
  })
  assert.equal(findCanvasNode(styled, 'canvas-messages').css, 'padding: 12px; color: var(--text);')
  assert.deepEqual(root, before)
})

test('containers containing required blocks must be emptied by moving them before deletion', () => {
  let root = addCanvasNode(fresh(), 'canvas-root', 'row')
  const row = lastChild(root).id
  root = moveCanvasNode(root, 'canvas-messages', row, 0)
  assert.throws(() => removeCanvasNode(root, row), { code: 'invalid_canvas' })
  root = moveCanvasNode(root, 'canvas-messages', root.id, 1)
  root = removeCanvasNode(root, row)
  assert.equal(findCanvasNode(root, row), undefined)
  assert.ok(canvasHasKind(root, 'messages'))
})

test('moves respect final sibling indices and reject cycles, missing nodes and invalid parents', () => {
  const root = fresh()
  const moved = moveCanvasNode(root, 'canvas-composer', root.id, 1)
  assert.deepEqual(
    moved.children.map((node) => node.kind),
    ['header', 'composer', 'messages'],
  )
  assert.deepEqual(
    moveCanvasNode(moved, 'canvas-composer', root.id, 3).children.map((node) => node.kind),
    ['header', 'messages', 'composer'],
  )
  for (const [node, parent, index] of [
    ['canvas-root', 'canvas-root', 0],
    ['canvas-messages', 'canvas-messages', 0],
    ['missing', 'canvas-root', 0],
    ['canvas-header', 'missing', 0],
    ['canvas-header', 'canvas-composer', 0],
    ['canvas-header', 'canvas-root', -1],
    ['canvas-header', 'canvas-root', 1.5],
    ['canvas-header', 'canvas-root', 99],
  ])
    assert.throws(() => moveCanvasNode(root, node, parent, index), { code: 'invalid_canvas' })
  let nested = addCanvasNode(root, root.id, 'column')
  const outer = lastChild(nested).id
  nested = addCanvasNode(nested, outer, 'row')
  const inner = findCanvasNode(nested, outer).children[0].id
  assert.throws(() => moveCanvasNode(nested, outer, inner, 0), { code: 'invalid_canvas' })
})

test('node count and nesting depth have explicit boundaries', () => {
  const root = fresh()
  while (root.children.length < CANVAS_MAX_NODES - 1)
    root.children.push({ id: `text-${root.children.length}`, kind: 'text', css: '', text: '' })
  assert.doesNotThrow(() => parseChatCanvas(root))
  assert.throws(() => addCanvasNode(root, root.id, 'spacer'), { code: 'invalid_canvas' })
  const deep = fresh()
  let nested = deep
  for (let depth = 2; depth <= CANVAS_MAX_DEPTH; depth++) {
    const child = { id: `depth-${depth}`, kind: 'column', css: '', children: [] }
    nested.children.push(child)
    nested = child
  }
  assert.doesNotThrow(() => parseChatCanvas(deep))
  nested.children.push({ id: 'too-deep', kind: 'text', css: '', text: '' })
  assert.throws(() => parseChatCanvas(deep), { code: 'invalid_canvas' })
})

test('untrusted node structure, ids, accessors and patches cannot alter the contract', () => {
  for (const value of [null, [], 'canvas', new Date(), { ...fresh(), script: 'bad' }])
    assert.throws(() => parseChatCanvas(value), { code: 'invalid_canvas' })
  for (const id of ['', '1bad', 'a b', '<script>', 'a'.repeat(65)])
    assert.throws(() => parseChatCanvas({ ...fresh(), id }), { code: 'invalid_canvas' })
  const duplicate = fresh()
  duplicate.children[1].id = duplicate.children[0].id
  assert.throws(() => parseChatCanvas(duplicate), { code: 'invalid_canvas' })
  const leaf = fresh()
  leaf.children[0].children = []
  assert.throws(() => parseChatCanvas(leaf), { code: 'invalid_canvas' })
  const missing = fresh()
  missing.children = missing.children.filter((node) => node.kind !== 'composer')
  assert.throws(() => parseChatCanvas(missing), { code: 'invalid_canvas' })
  assert.throws(() => updateCanvasNode(fresh(), 'canvas-header', { kind: 'text' }), {
    code: 'invalid_canvas',
  })
  assert.throws(() => updateCanvasNode(fresh(), 'canvas-header', { text: 'unexpected' }), {
    code: 'invalid_canvas',
  })
  let invoked = false
  const getter = fresh()
  Object.defineProperty(getter, 'css', {
    get() {
      invoked = true
      return ''
    },
  })
  assert.throws(() => parseChatCanvas(getter), { code: 'invalid_canvas' })
  assert.equal(invoked, false)
})

test('text length and node CSS validation also apply to editor updates', () => {
  const root = addCanvasNode(fresh(), 'canvas-root', 'text')
  const text = lastChild(root).id
  assert.equal(
    findCanvasNode(updateCanvasNode(root, text, { text: '<script>literal text</script>' }), text)
      .text,
    '<script>literal text</script>',
  )
  assert.doesNotThrow(() =>
    updateCanvasNode(root, text, { text: 'a'.repeat(CANVAS_TEXT_MAX_LENGTH) }),
  )
  assert.throws(
    () => updateCanvasNode(root, text, { text: 'a'.repeat(CANVAS_TEXT_MAX_LENGTH + 1) }),
    { code: 'invalid_canvas' },
  )
  assert.throws(() => updateCanvasNode(root, text, { css: 'background: url(remote)' }), {
    code: 'invalid_css',
  })
})
