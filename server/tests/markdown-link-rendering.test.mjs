import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import MarkdownMessage from '../../src/components/MarkdownMessage.tsx'

const ROOT = new URL('../../', import.meta.url)

test('chat Markdown links keep visible text and an explicit rendering class', () => {
  const html = renderToStaticMarkup(
    React.createElement(
      MarkdownMessage,
      null,
      'plain https://example.com/path\n\n[Documentation](https://example.com/docs)\n\n[](https://example.com/fallback)',
    ),
  )

  assert.match(html, /class="markdown-link"/)
  assert.match(html, />https:\/\/example\.com\/path<\/a>/)
  assert.match(html, />Documentation<\/a>/)
  assert.match(html, />https:\/\/example\.com\/fallback<\/a>/)
})

test('chat Markdown links force glyph fill to remain visible', async () => {
  const css = await readFile(new URL('src/index.css', ROOT), 'utf8')
  assert.match(css, /\.markdown-body \.markdown-link \{[^}]*-webkit-text-fill-color: currentColor;/s)
  assert.match(css, /text-decoration-thickness: 1px;/)
})
