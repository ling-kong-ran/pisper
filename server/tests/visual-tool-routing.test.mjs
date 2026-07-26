import { describe, it } from 'node:test'
import assert from 'node:assert'
import { hotToolNames } from '../tools/tool-activation.mjs'

const available = ['read', 'edit', 'web_search']

describe('visual-tool-routing', () => {
  it('hotToolNames returns only hot tools', () => {
    const result = hotToolNames(available)
    assert.deepEqual(result, ['read', 'edit'])
  })
})
