import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import test from 'node:test'

const root = resolve(import.meta.dirname, '../..')

test('runtime facade keeps one-way composition without HTTP route dependencies', async () => {
  const [runtime, facade] = await Promise.all(
    ['runtime/runtime/agent-runtime.mjs', 'runtime/runtime/agent-runtime-facade.mjs'].map((path) =>
      readFile(resolve(root, path), 'utf8'),
    ),
  )

  // 原行数预算试图限制入口职责增长；改为直接保护门面依赖方向和 HTTP 分层。
  assert.doesNotMatch(facade, /from ['"][^'"]*\/agent-runtime\.mjs['"]/)
  for (const source of [runtime, facade]) {
    assert.doesNotMatch(source, /(?:from|import\()\s*['"][^'"]*http\/routes\//)
    assert.doesNotMatch(source, /from ['"]node:http[s]?['"]/)
  }
  assert.match(runtime, /class AgentRuntimeService extends AgentRuntimeFacade/)
  assert.match(facade, /class AgentRuntimeFacade/)
})
