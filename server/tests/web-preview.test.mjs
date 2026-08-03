import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { normalizeWebPreviewInput, shouldOpenWebPreview } from '../../src/lib/web-preview.ts'
import {
  WEB_PREVIEW_PANEL_ID,
  webPreviewPanelTitle,
} from '../../src/features/chat/web-preview-panel.ts'

test('external http links open in the in-app preview while internal and special links keep native behavior', () => {
  const baseUrl = 'http://127.0.0.1:5173/chat'
  assert.equal(
    shouldOpenWebPreview({ href: 'https://example.com/docs', baseUrl }),
    'https://example.com/docs',
  )
  assert.equal(shouldOpenWebPreview({ href: '/config', baseUrl }), null)
  assert.equal(shouldOpenWebPreview({ href: 'mailto:hello@example.com', baseUrl }), null)
  assert.equal(
    shouldOpenWebPreview({ href: 'https://example.com/file', baseUrl, download: true }),
    null,
  )
  assert.equal(shouldOpenWebPreview({ href: 'https://example.com', baseUrl, ctrlKey: true }), null)
  assert.equal(
    shouldOpenWebPreview({
      href: 'https://example.com',
      baseUrl,
      behavior: 'external',
    }),
    null,
  )
})

test('URL bar input and dock panel metadata normalize preview destinations', () => {
  assert.equal(
    normalizeWebPreviewInput('example.com/guide', 'http://127.0.0.1:5173/chat'),
    'https://example.com/guide',
  )
  assert.equal(
    normalizeWebPreviewInput('https://example.com', 'http://127.0.0.1:5173/chat'),
    'https://example.com/',
  )
  assert.equal(normalizeWebPreviewInput('javascript:alert(1)', 'http://127.0.0.1:5173/chat'), null)
  assert.equal(WEB_PREVIEW_PANEL_ID, 'web-preview')
  assert.equal(webPreviewPanelTitle('https://www.reactbits.dev/docs'), 'reactbits.dev')
})

test('application routes external links into a right-side Dockview Web Preview panel', async () => {
  const [app, provider, chatPage, dockHook, dockPanel, component] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/components/WebPreviewProvider.tsx', 'utf8'),
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/use-chat-dock.ts', 'utf8'),
    readFile('src/features/chat/WebPreviewDockPanel.tsx', 'utf8'),
    readFile('src/components/ai-elements/web-preview.tsx', 'utf8'),
  ])

  assert.match(app, /<WebPreviewProvider \/>/)
  assert.match(provider, /requestWebPreview\(url\)/)
  assert.match(provider, /navigate\(pagePath\('chat'\)\)/)
  assert.match(chatPage, /webPreview: WebPreviewDockPanel/)
  assert.match(dockHook, /component: 'webPreview'/)
  assert.match(dockHook, /direction: 'right' as const/)
  assert.match(dockHook, /existing\.api\.updateParameters\(\{ url: request\.url \}\)/)
  assert.match(dockHook, /if \(sessionId\) setActiveId\(sessionId\)/)
  assert.match(dockHook, /panel\.api\.component !== 'session'/)
  assert.match(dockPanel, /window\.open\(currentUrl, '_blank', 'noopener,noreferrer'\)/)
  assert.match(dockPanel, /api\.onDidVisibilityChange/)
  assert.match(dockPanel, /if \(!visible\) return null/)
  assert.match(dockPanel, /<WebPreviewBody/)
  assert.match(
    component,
    /sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-presentation"/,
  )
  assert.match(component, /WebPreviewContext\.Provider/)
})
