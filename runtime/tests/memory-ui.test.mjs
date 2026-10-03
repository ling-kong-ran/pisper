import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('memory galaxy preserves its final dark background and four-point star styling', async () => {
  const source = await readFile('src/features/memory/components/MemoryGalaxy.tsx', 'utf8')
  // galaxy-panel 的样式以模板串书写（暂停动画的条件 class 拼接在尾部）。
  assert.match(source, /bg-\[var\(--galaxy-bg\)\]!/)
  assert.match(source, /galaxy-panel/)
  assert.match(source, /memory-star/)
  assert.match(source, /galaxy-star/)
})
