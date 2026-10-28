import assert from 'node:assert/strict'
import test from 'node:test'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { Button } from '../../src/components/ui/button.tsx'

// 这些断言保护可见 DOM 契约；交互与焦点由隔离浏览器夹具验收。
test('ordinary buttons do not submit forms unless explicitly requested', () => {
  const render = (props) => renderToStaticMarkup(createElement(Button, props, 'Save'))
  assert.match(render({}), /type="button"/)
  assert.match(render({ type: 'submit' }), /type="submit"/)
})

test('loading buttons expose busy and disabled state while retaining their sizing labels', () => {
  const html = renderToStaticMarkup(
    createElement(Button, { loading: true, sizingLabels: ['Save', 'Saving…', 'Saved'] }, 'Save'),
  )
  assert.match(html, /aria-busy="true"/)
  assert.match(html, /disabled=""/)
  assert.match(html, /Saving…/)
  assert.doesNotMatch(html, /sizingLabels=|loading=/)
})

test('disabled asChild links lose navigation and invalid button-only attributes', () => {
  const html = renderToStaticMarkup(
    createElement(
      Button,
      { asChild: true, disabled: true, type: 'submit' },
      createElement('a', { href: '/must-not-navigate' }, 'Open'),
    ),
  )
  assert.match(html, /^<a /)
  assert.match(html, /aria-disabled="true"/)
  assert.match(html, /tabindex="-1"/)
  assert.doesNotMatch(html, /href=| type=| disabled=/)
})

test('explanation-style disabled buttons remain keyboard reachable', () => {
  const html = renderToStaticMarkup(
    createElement(Button, { disabled: true, disabledMode: 'focusable' }, 'Unavailable'),
  )
  assert.match(html, /aria-disabled="true"/)
  assert.doesNotMatch(html, / disabled=/)
})

test('asChild native buttons inherit safe type and native disabled semantics', () => {
  const html = renderToStaticMarkup(
    createElement(Button, { asChild: true, loading: true }, createElement('button', null, 'Save')),
  )
  assert.match(html, /type="button"/)
  assert.match(html, / disabled=""/)
})
