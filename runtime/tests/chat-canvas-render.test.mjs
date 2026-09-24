import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import {
  ChatCanvasLayout,
  ChatCanvasSlot,
} from '../../src/features/chat/layout/ChatCanvasLayout.tsx'
import {
  chatCanvasNodeStyle,
  collectChatCanvasSlots,
  createChatCanvasHosts,
  collectFloatingCanvasIslands,
} from '../../src/features/chat/layout/chat-canvas-render.ts'
import { parseChatCanvas } from '../../src/features/chat/layout/chat-canvas.ts'

const leaf = (kind, id = kind) => ({ id, kind, css: '' })
const canvas = (children) => parseChatCanvas({ id: 'root', kind: 'column', css: '', children })
const marker = (name) => React.createElement('span', { 'data-test-block': name }, name)

test('canvas renders nested groups in document order and outputs text as escaped content', () => {
  const root = canvas([
    { ...leaf('text', 'caption'), text: '<script>unsafe()</script>' },
    {
      id: 'main',
      kind: 'row',
      css: 'gap: 12px;',
      children: [leaf('messages'), leaf('context')],
    },
    leaf('divider'),
    leaf('composer'),
  ])
  const html = renderToStaticMarkup(
    React.createElement(ChatCanvasLayout, {
      root,
      slots: {
        messages: marker('messages'),
        context: marker('context'),
        composer: marker('composer'),
      },
    }),
  )
  assert.match(html, /&lt;script&gt;unsafe\(\)&lt;\/script&gt;/)
  assert.doesNotMatch(html, /<script/)
  assert.ok(html.indexOf('data-test-block="messages"') < html.indexOf('data-test-block="context"'))
  assert.ok(html.indexOf('data-test-block="context"') < html.indexOf('data-test-block="composer"'))
  assert.match(html, /role="separator"/)
  assert.deepEqual(collectChatCanvasSlots(root), ['messages', 'context', 'composer'])
})

test('portable controls render once whether placed in the composer or as standalone blocks', () => {
  for (const detached of [false, true]) {
    const root = canvas([
      leaf('messages'),
      ...(detached ? [leaf('model'), leaf('tools')] : []),
      leaf('composer'),
    ])
    const slots = {
      messages: marker('messages'),
      model: marker('model'),
      tools: React.createElement(
        'section',
        { 'data-test-block': 'tools' },
        detached ? null : React.createElement(ChatCanvasSlot, { kind: 'model' }),
      ),
      composer: React.createElement(
        'form',
        { 'data-test-block': 'composer' },
        React.createElement('textarea', { defaultValue: 'unsent draft' }),
        detached ? null : React.createElement(ChatCanvasSlot, { kind: 'tools' }),
        React.createElement('button', { type: 'submit' }, 'Send'),
      ),
    }
    const html = renderToStaticMarkup(React.createElement(ChatCanvasLayout, { root, slots }))
    for (const kind of ['messages', 'model', 'tools', 'composer']) {
      assert.equal(html.match(new RegExp(`data-test-block="${kind}"`, 'g'))?.length, 1)
    }
    assert.match(html, /<textarea>unsent draft<\/textarea>/)
    assert.match(html, /<button type="submit">Send<\/button>/)
  }
})

test('canvas host identities survive node replacement and movement between containers', () => {
  let created = 0
  const getHost = createChatCanvasHosts((kind) => ({ kind, instance: ++created, draft: '' }))
  const original = getHost('composer')
  original.draft = 'local draft and attachments'
  const first = canvas([leaf('messages'), leaf('composer', 'first-composer')])
  const second = canvas([
    {
      id: 'new-container',
      kind: 'column',
      css: '',
      children: [leaf('composer', 'moved-composer')],
    },
    leaf('messages'),
  ])
  for (const root of [first, second, first]) {
    for (const kind of collectChatCanvasSlots(root)) getHost(kind)
    assert.equal(getHost('composer'), original)
    assert.equal(getHost('composer').draft, 'local draft and attachments')
  }
  assert.equal(created, 2)
})

test('canvas sizing preserves default growing message content and accepts local CSS overrides', () => {
  const root = canvas([leaf('header'), leaf('messages'), leaf('composer')])
  assert.equal(chatCanvasNodeStyle(root, true).height, '100%')
  assert.equal(chatCanvasNodeStyle(leaf('header')).flex, '0 0 auto')
  assert.equal(chatCanvasNodeStyle(leaf('messages')).flex, '1 1 0%')
  assert.equal(chatCanvasNodeStyle(leaf('composer')).flex, '0 0 auto')
  const grid = {
    id: 'grid',
    kind: 'grid',
    css: 'grid-template-columns: minmax(0, 2fr) minmax(180px, 1fr); gap: 16px; padding: 12px;',
    children: [leaf('messages'), leaf('composer')],
  }
  const style = chatCanvasNodeStyle(grid)
  assert.equal(style.display, 'grid')
  assert.equal(style.gridTemplateColumns, 'minmax(0, 2fr) minmax(180px, 1fr)')
  assert.equal(style.gap, '16px')
  assert.equal(style.padding, '12px')
  const sizedMessages = chatCanvasNodeStyle({ ...leaf('messages'), css: 'font-size: 125%;' })
  assert.equal(sizedMessages.fontSize, '125%')
  assert.equal(sizedMessages['--app-message-font-size'], '1em')
})

test('invalid duplicate functional slots are rejected before reaching the renderer', () => {
  assert.throws(() => canvas([leaf('messages'), leaf('composer'), leaf('composer', 'second')]))
})

test('custom UI nodes provide fillable host boxes without becoming singleton functional slots', () => {
  const widget = {
    id: 'widget-a',
    kind: 'custom-ui',
    componentId: 'weather-panel',
    css: 'height: 240px; flex-shrink: 0;',
  }
  const root = canvas([leaf('messages'), widget, { ...widget, id: 'widget-b' }, leaf('composer')])
  assert.deepEqual(collectChatCanvasSlots(root), ['messages', 'composer'])
  const style = chatCanvasNodeStyle(widget)
  assert.equal(style.display, 'flex')
  assert.equal(style.flexDirection, 'column')
  assert.equal(style.height, '240px')
  assert.equal(style.flexShrink, '0')
  assert.equal(style.flex, '0 0 auto')
  assert.equal(chatCanvasNodeStyle({ ...widget, css: 'height: 320px;' }).height, '320px')
})

test('built-in floating islands are collected separately and occupy no conversation flow space', () => {
  const island = {
    id: 'my-island',
    kind: 'custom-ui',
    componentId: 'pisper-island',
    css: 'height: 64px;',
  }
  const root = canvas([
    { id: 'group', kind: 'column', css: '', children: [island, leaf('messages')] },
    leaf('composer'),
  ])
  assert.deepEqual(collectFloatingCanvasIslands(root), [island])
  assert.deepEqual(collectFloatingCanvasIslands(canvas([leaf('messages'), leaf('composer')])), [])
  const html = renderToStaticMarkup(
    React.createElement(ChatCanvasLayout, {
      root,
      slots: { messages: marker('messages'), composer: marker('composer') },
    }),
  )
  assert.doesNotMatch(html, /data-canvas-node="my-island"/)
  assert.doesNotMatch(html, /height:64px/)
  assert.match(html, /data-test-block="messages"/)
  assert.match(html, /data-test-block="composer"/)
})
