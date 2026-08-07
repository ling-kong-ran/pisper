import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import test from 'node:test'

const root = resolve(import.meta.dirname, '../..')

test('runtime facade stays below its architecture budget', async () => {
  const [runtime, facade] = await Promise.all(
    ['runtime/runtime/agent-runtime.mjs', 'runtime/runtime/agent-runtime-facade.mjs'].map((path) =>
      readFile(resolve(root, path), 'utf8'),
    ),
  )

  assert.ok(runtime.split(/\r?\n/).length < 2500)
  assert.match(runtime, /class AgentRuntimeService extends AgentRuntimeFacade/)
  assert.match(facade, /class AgentRuntimeFacade/)
})
