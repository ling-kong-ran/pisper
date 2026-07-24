import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('Agent avatar avoids filtered SVG paint surfaces that corrupt after session tab switches', async () => {
  const [component, styles] = await Promise.all([
    readFile('src/components/AgentStatusAvatar.jsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.equal(component.includes('<filter'), false)
  assert.equal(component.includes('<feDropShadow'), false)
  assert.equal(component.includes('filter={`url(#'), false)
  assert.match(component, /className="agent-status-shadow"/)
  assert.match(styles, /\.agent-status-avatar \{[^}]*contain: paint;[^}]*overflow: hidden;/)
  assert.match(styles, /\.agent-status-avatar svg \{[^}]*overflow: hidden;/)
})
