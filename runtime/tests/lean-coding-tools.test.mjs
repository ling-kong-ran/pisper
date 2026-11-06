import assert from 'node:assert/strict'
import test from 'node:test'
import { Check } from 'typebox/value'
import {
  createBashTool,
  createEditTool,
  createFindTool,
  createGrepTool,
  createLsTool,
  createReadTool,
  createWriteTool,
} from '../runtime/pi-coding-agent.mjs'
import { LEAN_TOOL_TEXT, leanToolDefinition } from '../tools/lean-coding-tools.mjs'

const factories = {
  bash: createBashTool,
  read: createReadTool,
  edit: createEditTool,
  write: createWriteTool,
  grep: createGrepTool,
  find: createFindTool,
  ls: createLsTool,
}

function schemaPaths(schema, prefix = '') {
  if (schema?.items) return schemaPaths(schema.items, `${prefix}[]`)
  return Object.entries(schema?.properties ?? {}).flatMap(([key, child]) => {
    const path = prefix ? `${prefix}.${key}` : key
    return [path, ...schemaPaths(child, path)]
  })
}

test('every Pi coding tool has lean text, and the text matches its current parameters', async () => {
  assert.deepEqual(Object.keys(LEAN_TOOL_TEXT).sort(), Object.keys(factories).sort())
  for (const [name, create] of Object.entries(factories)) {
    const tool = await create('/repo')
    const paths = schemaPaths(tool.parameters)
    for (const path of Object.keys(LEAN_TOOL_TEXT[name].parameters))
      assert.ok(paths.includes(path), `${name}: Pi no longer has parameter ${path}`)
    for (const path of paths)
      assert.ok(path in LEAN_TOOL_TEXT[name].parameters, `${name}: no lean text for ${path}`)
  }
})

test('lean text changes the words, not the contract', async () => {
  for (const create of Object.values(factories)) {
    const tool = await create('/repo')
    const lean = leanToolDefinition(tool)
    assert.deepEqual(schemaPaths(lean.parameters), schemaPaths(tool.parameters))
    assert.deepEqual(lean.parameters.required, tool.parameters.required)
    assert.equal(lean.execute, tool.execute)
    assert.equal(lean.prepareArguments, tool.prepareArguments)
    assert.equal(lean.promptSnippet, undefined)
    assert.ok(lean.description.length < tool.description.length, tool.name)
    assert.ok(JSON.stringify(lean).length < JSON.stringify(tool).length, tool.name)
  }
})

test('the edit schema still accepts and rejects the same arguments', async () => {
  const { parameters } = leanToolDefinition(await createEditTool('/repo'))
  assert.equal(Check(parameters, { path: 'a', edits: [{ oldText: 'x', newText: 'y' }] }), true)
  assert.equal(Check(parameters, { path: 'a', edits: [{ oldText: 'x' }] }), false)
  assert.equal(Check(parameters, { edits: [] }), false)
})

test('the one constraint the edit tool must teach is still there, once', async () => {
  const lean = leanToolDefinition(await createEditTool('/repo'))
  const text = JSON.stringify(lean.parameters)
  assert.match(text, /occurs once in the file/)
  assert.match(text, /does not overlap/)
  assert.match(text, /matched against the original file/)
  assert.equal(text.split('overlap').length, 2)
})

test('tools without lean text pass through', () => {
  const tool = { name: 'other', description: 'x' }
  assert.equal(leanToolDefinition(tool), tool)
})
