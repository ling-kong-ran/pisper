import assert from 'node:assert/strict'
import test from 'node:test'
import { translateText } from '../../src/app/i18n.ts'
import {
  toolCapabilityLabel,
  toolCategoryLabel,
  toolDescription,
  toolName,
  toolRiskLabel,
  toolScopeLabel,
} from '../../src/features/plugins/tool-labels.ts'
import { TOOL_CATALOG } from '../tools/registry.mjs'

const CJK_PATTERN = /[\u3400-\u9fff]/

test('tool catalog labels and user-facing metadata have English translations', () => {
  const t = (key, values) => translateText(key, 'en-US', values)
  for (const tool of TOOL_CATALOG) {
    const labels = {
      name: toolName(tool, t),
      category: toolCategoryLabel(tool.category, t),
      risk: toolRiskLabel(tool.risk, t),
      description: toolDescription(tool, t),
      scope: toolScopeLabel(tool, t),
      capability: toolCapabilityLabel(tool, t),
    }
    for (const [field, value] of Object.entries(labels))
      assert.equal(CJK_PATTERN.test(value), false, `${tool.id}.${field} is not internationalized`)
  }
})
