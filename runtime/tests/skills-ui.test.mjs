import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('skill list items clamp long descriptions instead of growing with full text', async () => {
  const source = await readFile('src/features/skills/SkillsPage.tsx', 'utf8')

  // 左侧列表只承担摘要导航，完整描述应留在右侧详情区域
  assert.match(
    source,
    /<small className="line-clamp-2" title=\{skill\.description\}>[\s\S]*\{skill\.description\}/,
  )
})
