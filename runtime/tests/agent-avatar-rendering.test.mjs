import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('Agent avatar avoids filtered SVG paint surfaces that corrupt after session tab switches', async () => {
  const component = await readFile('src/components/AgentStatusAvatar.tsx', 'utf8')

  assert.equal(component.includes('<filter'), false)
  assert.equal(component.includes('<feDropShadow'), false)
  assert.equal(component.includes('filter={`url(#'), false)
  assert.match(component, /className="agent-status-shadow"/)
  assert.match(component, /agent-status-avatar[^"\n]*\[contain:paint\][^"\n]*overflow-hidden/)
  assert.match(component, /agent-status-avatar[^"\n]*\[&_svg\]:overflow-hidden/)
})
