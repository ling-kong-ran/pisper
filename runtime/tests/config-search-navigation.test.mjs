import assert from 'node:assert/strict'
import test from 'node:test'
import { searchConfig } from '../../src/features/config/config-search.ts'

for (const language of ['zh-CN', 'en-US']) {
  test(`appearance search targets the group containing the requested control (${language})`, () => {
    for (const query of ['font', '字体', '字号', 'corner', '圆角']) {
      const matches = searchConfig(query, language)
      assert.ok(
        matches.some(({ entry }) => entry.card === 'interface-style'),
        query,
      )
      assert.ok(!matches.some(({ entry }) => entry.card === 'interface-appearance'), query)
    }
    assert.ok(searchConfig('动效', language).some(({ entry }) => entry.card === 'interface-motion'))
    assert.ok(
      searchConfig('强调色', language).some(({ entry }) => entry.card === 'interface-appearance'),
    )
  })
}
