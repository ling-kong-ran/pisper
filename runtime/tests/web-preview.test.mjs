import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { normalizeWebPreviewInput, shouldOpenWebPreview } from '../../src/lib/web-preview.ts'
import {
  WEB_PREVIEW_PANEL_ID,
  webPreviewPanelTitle,
} from '../../src/features/chat/web-preview-panel.ts'
import { openWebPreviewInSessionContext } from '../../src/features/chat/session-web-preview.ts'
import { useContextPagesStore } from '../../src/features/chat/session-context-pages-store.ts'
import { previewSessionTarget } from '../../src/app/web-preview-target.ts'

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

test('application routes external links into the session inspector web preview', async () => {
  const [app, provider, chatPage, workspace, inspector, browserPane, component] = await Promise.all(
    [
      readFile('src/App.tsx', 'utf8'),
      readFile('src/app/WebPreviewProvider.tsx', 'utf8'),
      readFile('src/features/chat/ChatPage.tsx', 'utf8'),
      readFile('src/features/chat/SessionWorkspace.tsx', 'utf8'),
      readFile('src/features/chat/SessionInspector.tsx', 'utf8'),
      readFile('src/features/chat/SessionBrowserPane.tsx', 'utf8'),
      readFile('src/components/ai-elements/web-preview.tsx', 'utf8'),
    ],
  )

  assert.match(app, /<WebPreviewProvider \/>/)
  assert.match(provider, /requestWebPreview\(url\)/)
  // 不在会话页时带着请求去最近的有效活动会话，不再回旧的 /chat 多标签页；
  // 没有有效会话时在外部浏览器打开，绝不为了显示网页去 /board/new 新建空会话。
  assert.match(
    provider,
    /const sessionId = previewSessionTarget\(\s*pageStateStorage\.getItem\(STORAGE_KEYS\.activeSession\),\s*queryClient\.getQueryData<CachedSessions>\(SESSIONS_QUERY_KEY\),?\s*\)/,
  )
  assert.match(
    provider,
    /if \(!sessionId\) \{\s*window\.open\(url, '_blank', 'noopener,noreferrer'\)\s*return\s*\}\s*requestWebPreview\(url\)\s*navigate\(sessionPath\(sessionId\)\)/,
  )
  assert.doesNotMatch(provider, /sessionPath\([^)]*\|\| undefined\)|sessionPath\(\)/)
  assert.doesNotMatch(provider, /pagePath\('chat'\)/)
  // 会话页（单会话布局，没有 Dockview）：路由会话确认存在后才补开遗留请求，
  // 之后的预览事件落到当前会话检查器；“找不到这个会话”时请求不被吃掉。
  assert.match(
    chatPage,
    /if \(!routeTarget \|\| !routeKnown\) return\s+const pending = consumeWebPreviewRequest\(\)/,
  )
  assert.match(chatPage, /const pending = consumeWebPreviewRequest\(\)/)
  assert.match(
    chatPage,
    /if \(pending\) openWebPreviewInSessionContext\(routeTarget, pending\.url\)/,
  )
  assert.match(chatPage, /if \(url\) openWebPreviewInSessionContext\(routeTarget, url\)/)
  assert.match(chatPage, /addEventListener\(WEB_PREVIEW_OPEN_EVENT, openPreview\)/)
  // 第 3 期定案：桌面多标签 Dock 的右侧预览面板随 Dockview 一起退役（不再挂载），
  // 预览落到会话页检查器的“预览”标签。请求写进会话上下文页 store，检查器订阅它：
  // 浏览器页被激活或换了地址就切到“预览”标签并打开检查器。
  assert.match(workspace, /case 'browser':\s*return \{ tab: 'preview' \}/)
  assert.match(workspace, /useContextPagesStore\.subscribe\(\(state, previous\) => \{/)
  assert.match(workspace, /\(page\.kind === 'browser' && page\.url !== previousPage\?\.url\)/)
  assert.match(workspace, /if \(changed\) reveal\(page\.kind\)/)
  assert.match(inspector, /if \(tab === 'preview'\) return 'browser'/)
  assert.match(
    inspector,
    /<BrowserPane[\s\S]*?update\(sessionId, 'browser', \{ type: 'browser', id: page\.id, \.\.\.patch \}\)/,
  )
  // 检查器收起时整个面板不渲染：预览 iframe 只在“预览”标签可见时存在（取代 Dock 面板的可见性判断）。
  assert.match(workspace, /const inspector = open && \(/)
  assert.match(inspector, /\} else if \(tab === 'preview'\) \{/)
  assert.match(browserPane, /window\.open\(url, '_blank', 'noopener,noreferrer'\)/)
  assert.match(browserPane, /normalizeWebPreviewInput\(draft, window\.location\.href\)/)
  assert.match(browserPane, /<WebPreviewBody src=\{url\}/)
  assert.match(
    component,
    /sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-presentation"/,
  )
  assert.match(component, /WebPreviewContext\.Provider/)
})

test('link previews only target a live, unarchived session from the catalog cache', () => {
  const data = {
    sessions: [{ id: 'live' }, { id: 'archived', archived: true }],
  }
  assert.equal(previewSessionTarget('live', data), 'live')
  // 删除或归档的会话、从没打开过会话、目录还没加载：都没有目标，改为外部打开。
  assert.equal(previewSessionTarget('archived', data), '')
  assert.equal(previewSessionTarget('deleted', data), '')
  assert.equal(previewSessionTarget(null, data), '')
  assert.equal(previewSessionTarget('live', undefined), '')
})

test('session web preview reuses one browser page per session and moves it to the new address', () => {
  const browserPages = (sessionId) =>
    (useContextPagesStore.getState().sessions[sessionId]?.pages || []).filter(
      (page) => page.kind === 'browser',
    )
  assert.equal(openWebPreviewInSessionContext('', 'https://example.com/'), false)
  assert.equal(openWebPreviewInSessionContext('preview-session', ''), false)
  try {
    assert.equal(openWebPreviewInSessionContext('preview-session', 'https://example.com/a'), true)
    const [first] = browserPages('preview-session')
    assert.equal(first.url, 'https://example.com/a')
    assert.equal(useContextPagesStore.getState().sessions['preview-session'].activeId, first.id)
    assert.equal(openWebPreviewInSessionContext('preview-session', 'https://example.com/b'), true)
    const pages = browserPages('preview-session')
    assert.equal(pages.length, 1)
    assert.equal(pages[0].id, first.id)
    assert.equal(pages[0].url, 'https://example.com/b')
    // 预览只落在发起的会话里，不影响其他会话的检查器。
    assert.equal(browserPages('other-session').length, 0)
  } finally {
    useContextPagesStore.getState().forget('preview-session')
  }
})
