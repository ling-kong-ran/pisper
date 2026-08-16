import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('memory galaxy preserves its final dark background and four-point star styling', async () => {
  const source = await readFile('src/features/memory/MemoryPage.tsx', 'utf8')
  const graphPanel = source.match(/className="graph-panel[^"\n]*"/)?.[0] || ''
  const starCore = source.match(/className="star-core[^"\n]*"/)?.[0] || ''

  assert.match(graphPanel, /bg-\[var\(--galaxy-bg\)\]!/)
  assert.match(graphPanel, /backdrop-blur-none!/)
  assert.match(starCore, /bg-transparent/)
  assert.match(starCore, /shadow-\[none\]/)
  assert.match(starCore, /galaxy-star\.active_&[^\n]*scale\(1\.32\)/)
  assert.doesNotMatch(starCore, /radial-gradient\(circle_at_38%_34%/)
  assert.doesNotMatch(starCore, /scale\(1\.22\)|galaxy-star-pulse/)
  assert.match(source, /star-shape[^"\n]*fill:var\(--g-star-color\)/)
})
