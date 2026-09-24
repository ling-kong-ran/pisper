import assert from 'node:assert/strict'
import test from 'node:test'
import { DEFAULT_CHAT_LAYOUT } from '../../src/features/chat/layout/chat-layout.ts'
import {
  addCanvasNode,
  findCanvasNode,
  updateCanvasNode,
} from '../../src/features/chat/layout/chat-canvas.ts'
import {
  canCopyCanvasNode,
  canRemoveCanvasNode,
  canvasNodes,
  canvasParent,
  copyCanvasNode,
} from '../../src/features/chat/layout/chat-canvas-editor-model.ts'

function append(root, parentId, kind) {
  const next = addCanvasNode(root, parentId, kind)
  const added = findCanvasNode(next, parentId).children.at(-1)
  return { root: next, id: added.id }
}

test('editor copies nested decorative containers with fresh IDs, text and CSS', () => {
  let tree = structuredClone(DEFAULT_CHAT_LAYOUT.desktop.canvas)
  const row = append(tree, tree.id, 'row')
  const column = append(row.root, row.id, 'column')
  const text = append(column.root, column.id, 'text')
  tree = updateCanvasNode(text.root, text.id, {
    text: 'A personal note',
    css: 'color: #123456; padding: 12px;',
  })
  const original = structuredClone(tree)
  const duplicate = copyCanvasNode(tree, row.id)
  assert.deepEqual(tree, original)
  const source = findCanvasNode(duplicate, row.id)
  const copy = duplicate.children.at(-1)
  assert.notEqual(copy.id, source.id)
  assert.equal(copy.kind, 'row')
  assert.equal(copy.children[0].kind, 'column')
  assert.equal(copy.children[0].children[0].text, 'A personal note')
  assert.equal(copy.children[0].children[0].css, 'color: #123456; padding: 12px;')
  const ids = canvasNodes(duplicate).map((node) => node.id)
  assert.equal(ids.length, new Set(ids).size)
  assert.equal(canvasParent(duplicate, copy.children[0].children[0].id).id, copy.children[0].id)
})

test('editor prevents copying functional blocks or containers containing them', () => {
  const tree = structuredClone(DEFAULT_CHAT_LAYOUT.desktop.canvas)
  for (const node of canvasNodes(tree)) {
    if (['messages', 'composer', 'model', 'tools'].includes(node.kind)) {
      assert.equal(canCopyCanvasNode(node), false)
      assert.equal(copyCanvasNode(tree, node.id), tree)
    }
  }
  assert.equal(canCopyCanvasNode(tree), false)
  assert.equal(copyCanvasNode(tree, tree.id), tree)
})

test('editor copies custom UI instances and nested groups while retaining their component references', () => {
  const initial = structuredClone(DEFAULT_CHAT_LAYOUT.desktop.canvas)
  const group = append(initial, initial.id, 'column')
  let tree = addCanvasNode(group.root, group.id, 'custom-ui', 'weather-panel')
  const widget = findCanvasNode(tree, group.id).children[0]
  tree = updateCanvasNode(tree, widget.id, { css: 'height: 320px; border-radius: 12px;' })
  assert.equal(canCopyCanvasNode(findCanvasNode(tree, group.id)), true)
  assert.equal(canCopyCanvasNode(widget), true)
  tree = copyCanvasNode(tree, widget.id)
  const instances = findCanvasNode(tree, group.id).children
  assert.equal(instances.length, 2)
  for (const instance of instances) {
    assert.equal(instance.componentId, 'weather-panel')
    assert.equal(instance.css, 'height: 320px; border-radius: 12px;')
  }
  const copied = copyCanvasNode(tree, group.id)
  assert.equal(copied.children.at(-1).children.length, 2)
  for (const instance of copied.children.at(-1).children)
    assert.equal(instance.componentId, 'weather-panel')
  const ids = canvasNodes(copied).map((node) => node.id)
  assert.equal(ids.length, new Set(ids).size)
  assert.equal(findCanvasNode(tree, group.id).children.length, 2)
})

test('editor delete controls protect mandatory regions and their containing ancestors', () => {
  const tree = structuredClone(DEFAULT_CHAT_LAYOUT.desktop.canvas)
  for (const node of canvasNodes(tree)) {
    const required =
      node.id === tree.id ||
      canvasNodes(node).some((entry) => ['messages', 'composer'].includes(entry.kind))
    assert.equal(canRemoveCanvasNode(tree, node), !required)
  }
  const decorative = append(tree, tree.id, 'column')
  assert.equal(
    canRemoveCanvasNode(decorative.root, findCanvasNode(decorative.root, decorative.id)),
    true,
  )
})

test('quick styles preserve quoted CSS delimiters and replace repeated declarations without growth', async () => {
  const { replaceCanvasCssDeclaration } =
    await import('../../src/features/chat/layout/chat-canvas-editor-model.ts')
  let css = 'font-family: "A; color: red"; color: blue; color: green;'
  css = replaceCanvasCssDeclaration(css, 'color', '#123456')
  assert.equal(css, 'font-family: "A; color: red";\ncolor: #123456;')
  assert.equal(replaceCanvasCssDeclaration(css, 'color', '#123456'), css)
  assert.match(replaceCanvasCssDeclaration(css, 'padding', '12px'), /font-family: "A; color: red"/)
})
