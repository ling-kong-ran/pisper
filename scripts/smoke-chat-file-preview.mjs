import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const origin = process.argv[2] || 'http://127.0.0.1:5173'
const screenshots = await mkdtemp(join(tmpdir(), 'pisper-file-preview-smoke-'))
const fixture = await build({
  stdin: {
    contents: `
      import React from 'react'
      import { createRoot } from 'react-dom/client'
      import { MessageAttachments } from './src/features/chat/ChatMessage.tsx'
      import { LOCAL_REVEAL_NOTICE_EVENT } from './src/app/route-context.ts'
      window.notices = []
      window.addEventListener(LOCAL_REVEAL_NOTICE_EVENT, event => window.notices.push(event.detail))
      createRoot(document.getElementById('root')).render(React.createElement(MessageAttachments, {
        sessionId: 'preview-smoke',
        attachments: [
          {id:'notes',kind:'file',name:'notes.md',path:'/workspace/notes.md',mimeType:'text/markdown',downloadUrl:'/api/assets/notes/download'},
          {id:'code',kind:'file',name:'index.html',path:'/workspace/index.html',mimeType:'text/html',downloadUrl:'/api/assets/code/download'},
          {id:'binary',kind:'file',name:'archive.zip',mimeType:'application/zip',downloadUrl:'/api/assets/binary/download'},
          {id:'missing',kind:'file',name:'missing.txt',downloadUrl:'/api/assets/missing/download'},
          {kind:'file',name:'inline.txt',text:'Inline attachment content'},
        ],
      }))
    `,
    resolveDir: root,
    loader: 'tsx',
  },
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'browser',
  alias: { '@': join(root, 'src'), '@shared': join(root, 'shared') },
  define: { 'process.env.NODE_ENV': '"production"' },
  loader: { '.css': 'empty' },
})
const executablePath =
  process.env.PISPER_SMOKE_BROWSER ||
  [
    chromium.executablePath(),
    '/Applications/Google Chrome Dev.app/Contents/MacOS/Google Chrome Dev',
  ].find(existsSync)
assert(executablePath, 'Set PISPER_SMOKE_BROWSER to a Chromium executable.')
const browser = await chromium.launch({ executablePath, headless: true })
try {
  for (const width of [1440, 390, 320]) {
    const page = await browser.newPage({ viewport: { width, height: 900 } })
    const errors = []
    page.on('pageerror', (error) => errors.push(error.message))
    let missingAttempts = 0
    let diffMode = 'empty'
    // 页面和 API 均使用临时样本，不读取或改写用户会话与资产。
    await page.route('**/__attachment-preview-smoke__', (route) =>
      route.fulfill({
        contentType: 'text/html',
        body: '<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><div id="root" style="padding:16px"></div><link rel="stylesheet" href="/src/index.css?direct"><script type="module" src="/__attachment-preview-fixture.js"></script></body></html>',
      }),
    )
    await page.route('**/__attachment-preview-fixture.js', (route) =>
      route.fulfill({ contentType: 'text/javascript', body: fixture.outputFiles[0].text }),
    )
    await page.route('**/api/assets/*/content?preview=1', (route) => {
      const id = new URL(route.request().url()).pathname.split('/')[3]
      if (id === 'missing' && missingAttempts++ === 0)
        return route.fulfill({ status: 404, json: { error: 'Preview asset not found' } })
      return route.fulfill({
        json:
          id === 'binary'
            ? { kind: 'file' }
            : {
                kind: 'text',
                mimeType: id === 'notes' ? 'text/markdown' : 'text/plain',
                text:
                  id === 'notes'
                    ? '# Preview heading\n\nCurrent **file** content.'
                    : id === 'code'
                      ? '<script>window.__previewExecuted = true</script>\n' +
                        'long line '.repeat(80)
                      : 'Retry succeeded',
                truncated: id === 'code',
              },
      })
    })
    await page.route('**/api/sessions/preview-smoke/vcs/file-diff?*', (route) =>
      route.fulfill({
        json: {
          isRepo: false,
          source: 'snapshot',
          diff:
            diffMode === 'empty'
              ? ''
              : 'diff --git a/notes.md b/notes.md\n--- a/notes.md\n+++ b/notes.md\n@@ -1 +1 @@\n-before\n+after\n',
          canRevert: diffMode !== 'empty',
        },
      }),
    )
    await page.goto(`${origin}/__attachment-preview-smoke__`)
    const openPreview = async (name) => {
      await page.getByRole('button', { name, exact: true }).click()
      await page
        .locator('[data-slot="popover-content"][data-state="open"]')
        .getByRole('button', { name: '预览', exact: true })
        .click()
      await page.locator('[data-slot="dialog-content"]').waitFor()
      await page.locator('[data-slot="popover-content"]').waitFor({ state: 'detached' })
    }
    await openPreview('notes.md')
    await page.getByRole('heading', { name: 'Preview heading', exact: true }).waitFor()
    await page.getByRole('tab', { name: '源码', exact: true }).click()
    await page.getByText('# Preview heading', { exact: false }).waitFor()
    await page.getByRole('tab', { name: '预览', exact: true }).click()
    await page.screenshot({ path: join(screenshots, `markdown-${width}.png`) })
    await page.keyboard.press('Escape')
    await page.locator('[data-slot="dialog-content"]').waitFor({ state: 'detached' })
    assert.equal(
      await page
        .getByRole('button', { name: 'notes.md', exact: true })
        .evaluate((el) => document.activeElement === el),
      true,
    )
    await page.getByRole('button', { name: 'notes.md', exact: true }).click()
    await page.getByRole('button', { name: '查看改动', exact: true }).click()
    await page.waitForFunction(() => window.notices.length > 0)
    assert.match(await page.evaluate(() => window.notices.at(-1).message), /没有可显示的改动/)
    diffMode = 'changes'
    await page.getByRole('button', { name: '查看改动', exact: true }).click()
    await page.locator('.git-diff-dialog').waitFor()
    await page.getByText('after', { exact: true }).waitFor()
    await page.keyboard.press('Escape')
    await openPreview('index.html')
    await page.getByText('内容过长，仅显示部分。', { exact: true }).waitFor()
    assert.equal(await page.evaluate(() => window.__previewExecuted), undefined)
    const bounds = await page.locator('[data-slot="dialog-content"]').evaluate((el) => ({
      width: el.getBoundingClientRect().width,
      overflow: el.scrollWidth > el.clientWidth,
    }))
    assert(bounds.width <= width - 30)
    assert.equal(bounds.overflow, false)
    await page.screenshot({ path: join(screenshots, `code-${width}.png`) })
    await page.keyboard.press('Escape')
    await openPreview('archive.zip')
    await page.getByText('此文件暂不支持内容预览。', { exact: true }).waitFor()
    await page.keyboard.press('Escape')
    await openPreview('missing.txt')
    await page.getByRole('alert').filter({ hasText: 'Preview asset not found' }).waitFor()
    await page.getByRole('button', { name: '重试', exact: true }).click()
    await page.getByText('Retry succeeded', { exact: true }).waitFor()
    await page.keyboard.press('Escape')
    await openPreview('inline.txt')
    await page.getByText('Inline attachment content', { exact: true }).waitFor()
    assert.deepEqual(errors, [])
    console.log(
      `PASS ${width}px: preview, Markdown/source, escaped HTML, truncation, binary, retry, numeric key, diff, focus`,
    )
    await page.close()
  }
  console.log(`Screenshots: ${screenshots}`)
} finally {
  await browser.close()
}
