import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('skill list items clamp long descriptions instead of growing with full text', async () => {
  const [source, template] = await Promise.all([
    readFile('src/features/skills/SkillsPage.tsx', 'utf8'),
    readFile('src/components/app/FeaturePage.tsx', 'utf8'),
  ])

  // 列表行只承担摘要导航（功能页模板的行说明最多两行），完整描述留在行下方展开的详情里（A6 取代右侧检查器）。
  assert.match(
    source,
    /<FeatureRow[\s\S]*?description=\{skill\.description\}[\s\S]*?detail=\{renderSkillDetail\(skill\)\}/,
  )
  assert.match(
    template,
    // 说明 span 带 id（A6 评审返修：展开键用 aria-describedby 指向它），仍是最多两行。
    /\{description && \(\s*<span\s+id=\{descriptionId\}\s+className="line-clamp-2 [^"]*"\s*>\s*\{description\}/,
  )
  const detail = source.slice(source.indexOf('const renderSkillDetail'))
  assert.match(detail, /whitespace-pre-line text-content-soft">\s*\{skill\.description \|\|/)
  assert.doesNotMatch(detail.slice(0, detail.indexOf('</p>')), /line-clamp|truncate/)
})
