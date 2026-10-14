import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import test from 'node:test'

// 脚本是纯 Node ESM CLI，这里按仓库惯例用 process.execPath 直接 spawn 验证。
function runSourceGuards(args) {
  return spawnSync(process.execPath, ['scripts/list-source-guards.mjs', ...args], {
    cwd: process.cwd(),
    encoding: 'utf8',
  })
}

test('default mode lists guarded source files grouped by test file', () => {
  const result = runSourceGuards([])
  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /runtime\/tests\/chat-stream-rendering\.test\.mjs:/)
  // 汇总统计必须同时覆盖测试数与源文件数。
  assert.match(result.stdout, /Summary: \d+ test files guard \d+ source files\./)
})

test('--source reverse lookup finds every test reading the given source file', () => {
  const result = runSourceGuards(['--source', 'src/features/chat/AgentRunActivity.tsx'])
  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /^runtime\/tests\/chat-stream-rendering\.test\.mjs$/m)
})
