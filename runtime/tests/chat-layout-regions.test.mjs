import assert from 'node:assert/strict'
import test from 'node:test'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { SessionContextLayout } from '../../src/features/chat/SessionContextLayout.tsx'
import { parseChatLayoutJson } from '../../src/features/chat/layout/chat-layout.ts'
import { readFile } from 'node:fs/promises'

function render(presentation, side = 'right') {
  return renderToStaticMarkup(
    createElement(
      SessionContextLayout,
      {
        presentation,
        side,
        availableWidth: 1400,
        context: createElement('aside', { 'data-test-region': 'context' }),
      },
      createElement('main', { 'data-test-region': 'conversation' }),
    ),
  )
}

test('context position changes the reading order while keeping both regions accessible', () => {
  for (const side of ['left', 'right']) {
    const html = render('aside', side)
    const conversation = html.indexOf('data-test-region="conversation"')
    const context = html.indexOf('data-test-region="context"')
    assert.ok(conversation >= 0 && context >= 0)
    assert.equal(context < conversation, side === 'left')
    assert.match(html, /role="separator"/)
    assert.match(html, /tabindex="0"/)
  }
})

test('closed and compact contexts leave the conversation available without a desktop resize handle', () => {
  const closed = render('closed', 'left')
  assert.match(closed, /data-test-region="conversation"/)
  assert.doesNotMatch(closed, /data-test-region="context"|role="separator"/)
  const compact = render('sheet', 'left')
  assert.match(compact, /data-test-region="conversation"/)
  assert.match(compact, /data-test-region="context"/)
  assert.doesNotMatch(compact, /role="separator"/)
})

test('the documented template can be imported with independent phone options', async () => {
  const layout = parseChatLayoutJson(await readFile('docs/chat-layout.example.json', 'utf8'))
  assert.equal(layout.version, 2)
  assert.equal(layout.desktop.navigationSide, 'left')
  assert.equal(layout.mobile.composerPosition, 'bottom')
  assert.ok(!('navigationSide' in layout.mobile))
  assert.deepEqual(
    layout.desktop.canvas.children.map((node) => node.kind),
    ['header', 'messages', 'composer'],
  )
  assert.notEqual(layout.mobile.canvas, layout.desktop.canvas)
})
