import assert from 'node:assert/strict'
import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer } from 'vite'
import { chromium } from 'playwright-core'

const output = await mkdtemp(join(tmpdir(), 'pisper-replica-assets-'))
const asset = (id, name = id, extra = {}) => ({
  id,
  name,
  kind: 'file',
  mimeType: 'text/plain',
  size: 12,
  source: 'attachment',
  ...extra,
})
const specimens = [
  asset('slow', 'Slow text'),
  asset('plain', 'Long plain'),
  asset('markdown', 'Notes.md', { mimeType: 'text/markdown' }),
  asset('picture', 'Picture', { kind: 'image', mimeType: 'image/png' }),
  asset('binary', 'Binary', { mimeType: 'application/octet-stream' }),
  asset('video', 'Video', { mimeType: 'video/mp4' }),
  asset('link', 'Website', {
    kind: 'link',
    mimeType: 'text/uri-list',
    url: 'https://example.com/',
  }),
]
specimens.push(
  asset('long', 'Long title ' + 'verylongname'.repeat(60)),
  ...Array.from({ length: 12 }, (_, i) => asset('extra-' + i, 'Result ' + i)),
)
const errors = []
const metrics = []
const server = await createServer({
  server: { host: '127.0.0.1', port: 0, hmr: false, watch: null },
  appType: 'custom',
  plugins: [
    {
      name: 'replica-assets-fixture',
      configureServer(server) {
        server.middlewares.use(async (req, res, next) => {
          const path = new URL(req.url, 'http://fixture').pathname
          if (path === '/__assets') {
            res.setHeader('Content-Type', 'text/html; charset=utf-8')
            res.end(
              await server.transformIndexHtml(
                '/__assets',
                '<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><div id="root"></div><script type="module" src="/scripts/fixtures/replica-assets.tsx"></script></body></html>',
              ),
            )
          } else if (/^\/api\/assets\/[^/]+\/download$/.test(path)) {
            res.setHeader('Content-Type', 'application/octet-stream')
            const id = decodeURIComponent(path.split('/')[3])
            const name = specimens.find((item) => item.id === id)?.name || id
            res.setHeader(
              'Content-Disposition',
              `attachment; filename*=UTF-8''${encodeURIComponent(name)}`,
            )
            res.end('fixture bytes')
          } else next()
        })
      },
    },
  ],
})
let browser
function deferred() {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}
try {
  await server.listen()
  browser = await chromium.launch({ headless: true })
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, locale: 'en-US' })
  page.setDefaultTimeout(10000)
  page.on('pageerror', (error) => errors.push(error.message))
  await page.addInitScript(() => localStorage.setItem('pisper-language', 'en-US'))
  const slow = deferred(),
    started = deferred(),
    finished = deferred()
  let listFailure = false
  let listEmpty = false
  let scopeEmpty = false
  let pictureFailure = true
  let scopeSlow = false
  const scopeStarted = deferred(),
    scopeRelease = deferred(),
    scopeFinished = deferred()
  const deleted = new Set()
  await page.route('**/api/assets/*/download*', (route) => {
    const id = new URL(route.request().url()).pathname.split('/')[3]
    if (!new URL(route.request().url()).searchParams.has('inline'))
      return route.fulfill({
        contentType: 'application/octet-stream',
        headers: { 'Content-Disposition': 'attachment; filename=fixture.txt' },
        body: 'fixture bytes',
      })
    if (id === 'picture' && !pictureFailure)
      return route.fulfill({
        contentType: 'image/png',
        body: Buffer.from(
          'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=',
          'base64',
        ),
      })
    return route.fulfill({ status: 503, body: 'Synthetic media failure' })
  })
  await page.route('**/api/assets?*', async (route) => {
    const url = new URL(route.request().url())
    const query = url.searchParams.get('query')
    if (listFailure || query === 'error')
      return route.fulfill({ status: 503, json: { error: 'Synthetic list failure' } })
    if (scopeEmpty && url.searchParams.get('sessionId'))
      return route.fulfill({ json: { assets: [] } })
    if (scopeSlow && url.searchParams.get('sessionId') === 'a') {
      scopeStarted.resolve()
      await scopeRelease.promise
      await route
        .fulfill({ json: { assets: [asset('scope-a', 'Scope A result')] } })
        .catch(() => {})
      scopeFinished.resolve()
      return
    }
    if (scopeSlow && url.searchParams.get('sessionId') === 'b')
      return route.fulfill({ json: { assets: [asset('scope-b', 'Scope B result')] } })
    if (query === 'slow') {
      started.resolve()
      await slow.promise
      await route.fulfill({ json: { assets: [asset('old', 'Obsolete result')] } }).catch(() => {})
      finished.resolve()
      return
    }
    return route.fulfill({
      json: {
        assets:
          query === 'fast'
            ? [asset('fast', 'Current result')]
            : query === 'empty' || listEmpty
              ? []
              : specimens.filter(
                  (item) =>
                    !deleted.has(item.id) &&
                    (!url.searchParams.get('kind') || item.kind === url.searchParams.get('kind')),
                ),
      },
    })
  })
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/__assets`, {
    waitUntil: 'domcontentloaded',
    timeout: 45000,
  })
  await page
    .getByRole('button', { name: 'Preview: Slow text', exact: true })
    .waitFor({ timeout: 45000 })
  const search = page.getByRole('textbox', { name: 'Fixture search' })
  await search.fill('slow')
  await started.promise
  await search.fill('fast')
  await page.getByText('Current result', { exact: true }).waitFor()
  slow.resolve()
  await finished.promise
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  assert.equal(
    await page.getByText('Current result', { exact: true }).isVisible(),
    true,
    'obsolete result must not replace the current query',
  )
  assert.equal(await page.getByText('Obsolete result', { exact: true }).count(), 0)
  await search.fill('')
  await page.getByRole('button', { name: 'Preview: Slow text', exact: true }).waitFor()
  await page.getByRole('button', { name: 'Filters', exact: true }).click()
  await page.getByRole('menuitemradio', { name: 'Images', exact: true }).click()
  assert.equal(await page.getByRole('menu').isVisible(), true)
  await page.getByRole('menuitemcheckbox', { name: 'From current chat', exact: true }).click()
  assert.equal(await page.getByRole('menu').isVisible(), true)
  await page.keyboard.press('Escape')
  await page.getByRole('button', { name: 'Remove Images filter', exact: true }).click()
  await page.getByRole('button', { name: 'No session', exact: true }).click()
  await page.getByText('Select a conversation to view its assets.', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'Remove From current chat filter', exact: true }).click()
  // 首次失败与无匹配分别呈现；同键刷新失败保留可操作结果。
  await search.fill('error')
  await page.getByRole('alert').getByText('Synthetic list failure', { exact: true }).waitFor()
  assert.equal(
    await page.getByRole('heading', { name: 'Nothing has gathered here yet', exact: true }).count(),
    0,
  )
  assert.equal(await page.locator('[data-asset-id]').count(), 0)
  await search.fill('empty')
  await page.getByRole('heading', { name: 'No matching assets', exact: true }).waitFor()
  await search.fill('')
  await page.getByRole('button', { name: 'Preview: Long plain', exact: true }).waitFor()
  listFailure = true
  await page.getByRole('button', { name: 'Refresh assets', exact: true }).click()
  await page.getByRole('alert').getByText('Synthetic list failure', { exact: true }).waitFor()
  assert.equal(
    await page.getByRole('button', { name: 'Preview: Long plain', exact: true }).isVisible(),
    true,
  )
  listFailure = false
  await page.getByRole('button', { name: 'Retry', exact: true }).click()
  await page.getByRole('alert').waitFor({ state: 'hidden' })
  // 相同 current 条件下切活动会话，A 迟到不能进入 B。
  await page.getByRole('button', { name: 'Session A', exact: true }).click()
  scopeSlow = true
  await page.getByRole('button', { name: 'Filters', exact: true }).click()
  await page.getByRole('menuitemcheckbox', { name: 'From current chat', exact: true }).click()
  await page.keyboard.press('Escape')
  await scopeStarted.promise
  await page.getByRole('button', { name: 'Session B', exact: true }).click()
  await page.getByText('Scope B result', { exact: true }).waitFor()
  scopeRelease.resolve()
  await scopeFinished.promise
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  assert.equal(await page.getByText('Scope A result', { exact: true }).count(), 0)
  assert.equal(await page.getByText('Scope B result', { exact: true }).isVisible(), true)
  scopeSlow = false
  scopeEmpty = true
  await page.getByRole('button', { name: 'Refresh assets', exact: true }).click()
  await page
    .getByRole('heading', { name: 'No matching assets in this conversation', exact: true })
    .waitFor()
  scopeEmpty = false
  await page.getByRole('button', { name: 'Remove From current chat filter', exact: true }).click()
  const previewStarted = deferred(),
    previewRelease = deferred()
  await page.route('**/api/assets/*/content?preview=1', async (route) => {
    const id = new URL(route.request().url()).pathname.split('/')[3]
    if (id === 'slow') {
      previewStarted.resolve()
      await previewRelease.promise
    }
    return route
      .fulfill({
        json:
          id === 'binary'
            ? asset(id, 'Binary')
            : {
                ...asset(id),
                kind: 'text',
                text:
                  id === 'markdown'
                    ? '# Preview heading\n\nMarkdown body'
                    : ('Plain line \n' + 'longword'.repeat(60) + '\n').repeat(100),
              },
      })
      .catch(() => {})
  })
  const previewSlow = page.getByRole('button', { name: 'Preview: Slow text', exact: true })
  await previewSlow.click()
  await previewStarted.promise
  const dialog = page.getByRole('dialog')
  await dialog.getByRole('status').waitFor()
  await page.keyboard.press('Escape')
  await dialog.waitFor({ state: 'hidden' })
  await previewSlow.evaluate((element) => {
    if (document.activeElement !== element) throw new Error('preview focus did not return')
  })
  previewRelease.resolve()
  await page.getByRole('button', { name: 'Preview: Long plain', exact: true }).click()
  await dialog.locator('[data-asset-plain]').waitFor()
  assert.equal(
    await dialog
      .locator('[data-asset-plain]')
      .evaluate((element) => getComputedStyle(element).whiteSpace),
    'pre-wrap',
  )
  for (let index = 0; index < 12; index++) {
    await page.keyboard.press('Tab')
    assert.equal(await dialog.evaluate((element) => element.contains(document.activeElement)), true)
  }
  await page.keyboard.press('Escape')
  await dialog.waitFor({ state: 'hidden' })
  await previewSlow.click()
  await dialog.locator('[data-asset-plain]').waitFor()
  await page.evaluate(() => window.assetFixture.setQuery('empty'))
  await page.locator('[data-asset-id]').waitFor({ state: 'hidden' })
  await dialog.getByRole('button', { name: 'Close dialog', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  await page.waitForFunction(() => document.activeElement?.textContent?.trim() === 'Filters')
  await search.fill('')
  await page.getByRole('button', { name: 'Preview: Notes.md', exact: true }).waitFor()
  await page.getByRole('button', { name: 'Preview: Notes.md', exact: true }).click()
  await dialog.getByRole('heading', { name: 'Preview heading', exact: true }).waitFor()
  await page.keyboard.press('Escape')
  await dialog.waitFor({ state: 'hidden' })
  await page.getByRole('button', { name: 'Preview: Binary', exact: true }).click()
  await dialog.getByText('This file type can be downloaded.', { exact: false }).waitFor()
  await dialog.getByRole('button', { name: 'Download', exact: true }).waitFor()
  await page.keyboard.press('Escape')
  await dialog.waitFor({ state: 'hidden' })
  let previewFailed = true
  await page.route('**/api/assets/binary/content?preview=1', (route) =>
    previewFailed
      ? route.fulfill({ status: 503, json: { error: 'Synthetic preview failure' } })
      : route.fulfill({ json: asset('binary') }),
  )
  const binaryTrigger = page.getByRole('button', { name: 'Preview: Binary', exact: true })
  await binaryTrigger.click()
  await dialog.getByRole('alert').getByText('Synthetic preview failure', { exact: true }).waitFor()
  previewFailed = false
  await dialog.getByRole('button', { name: 'Retry', exact: true }).click()
  await dialog.getByText('This file type can be downloaded.', { exact: false }).waitFor()
  await page.locator('[data-slot="dialog-overlay"]').click({ position: { x: 5, y: 5 } })
  await dialog.waitFor({ state: 'hidden' })
  await page.waitForFunction(
    () => document.activeElement?.getAttribute('aria-label') === 'Preview: Binary',
  )
  // 视频使用真实video/controls/playsInline；不依赖机器是否安装对应解码器。
  const videoStarted = deferred(),
    videoRelease = deferred(),
    videoFinished = deferred()
  await page.route('**/api/assets/video/download?inline=1', async (route) => {
    videoStarted.resolve()
    await videoRelease.promise
    await route.fulfill({ status: 503, body: 'Synthetic video decode failure' }).catch(() => {})
    videoFinished.resolve()
  })
  await page.getByRole('button', { name: 'Preview: Video', exact: true }).click()
  await videoStarted.promise
  assert.equal(await dialog.locator('video').getAttribute('controls'), '')
  assert.equal(await dialog.locator('video').getAttribute('playsinline'), '')
  await page.keyboard.press('Escape')
  await dialog.waitFor({ state: 'hidden' })
  videoRelease.resolve()
  await videoFinished.promise
  await page.getByRole('button', { name: 'Preview: Picture', exact: true }).click()
  await dialog.getByRole('alert').getByText('Media could not be loaded', { exact: true }).waitFor()
  assert.equal(await dialog.getByRole('button', { name: /previous|next/i }).count(), 0)
  pictureFailure = false
  await dialog.getByRole('button', { name: 'Retry', exact: true }).click()
  await page.waitForFunction(() => {
    const image = document.querySelector('[role="dialog"] img')
    return image?.complete && image.naturalWidth > 0
  })
  await page.keyboard.press('Escape')
  await dialog.waitFor({ state: 'hidden' })
  // 显式加入聊天的完整读取由当前会话拥有，切换后不能投递旧回执。
  const useStarted = deferred(),
    useRelease = deferred(),
    useFinished = deferred()
  let uses = 0
  await page.route('**/api/assets/plain/content', async (route) => {
    uses++
    useStarted.resolve()
    await useRelease.promise
    await route
      .fulfill({ json: { ...asset('plain'), kind: 'text', text: 'Complete attachment' } })
      .catch(() => {})
    useFinished.resolve()
  })
  const plainCard = page.locator('[data-asset-id="plain"]')
  await plainCard.getByRole('button', { name: 'Use in chat', exact: true }).evaluate((element) => {
    element.click()
    element.click()
  })
  await useStarted.promise
  assert.equal(
    await plainCard.getByRole('button', { name: 'Use in chat', exact: true }).isDisabled(),
    true,
  )
  await page.getByRole('button', { name: 'Session A', exact: true }).click()
  useRelease.resolve()
  await useFinished.promise
  await page.waitForFunction(
    () =>
      document.querySelector('[data-asset-id="plain"] [data-slot="button"][aria-busy]') === null,
  )
  assert.equal(uses, 1)
  assert.equal(await page.evaluate(() => window.assetEvidence.used.length), 0)
  await page.unroute('**/api/assets/plain/content')
  await page.route('**/api/assets/plain/content', (route) =>
    route.fulfill({ json: { ...asset('plain'), kind: 'text', text: 'Complete attachment' } }),
  )
  await plainCard.getByRole('button', { name: 'Use in chat', exact: true }).click()
  await page.waitForFunction(() => window.assetEvidence.used.length === 1)
  assert.equal(await page.evaluate(() => window.assetEvidence.used[0].text), 'Complete attachment')
  const download = page.waitForEvent('download')
  await plainCard.getByRole('button', { name: 'Download', exact: true }).click()
  const downloaded = await download
  assert.ok(downloaded.suggestedFilename().startsWith('Long plain'))
  assert.equal(await readFile(await downloaded.path(), 'utf8'), 'fixture bytes')
  await page.evaluate(() => {
    window.nativeAssetEvidence = []
    window.pisperDesktop = {
      openAsset: (input) => {
        window.nativeAssetEvidence.push(input)
        return Promise.resolve()
      },
    }
  })
  await search.fill(' ')
  await plainCard.getByRole('button', { name: 'Open in app', exact: true }).click()
  await page.waitForFunction(() => window.nativeAssetEvidence.length === 1)
  assert.equal(await page.evaluate(() => window.nativeAssetEvidence[0].name), 'Long plain')
  assert.equal(await page.evaluate(() => atob(window.nativeAssetEvidence[0].data)), 'fixture bytes')
  await page.evaluate(() => {
    delete window.pisperDesktop
  })
  await search.fill('')
  const deleteStarted = deferred(),
    deleteRelease = deferred()
  let deletes = 0
  await page.route('**/api/assets/binary', async (route) => {
    deletes++
    deleteStarted.resolve()
    await deleteRelease.promise
    deleted.add('binary')
    return route.fulfill({ json: { deleted: true } })
  })
  const deleteButton = page.getByRole('button', { name: 'Delete asset: Binary', exact: true })
  await deleteButton.evaluate((element) => {
    element.click()
    element.click()
  })
  await deleteStarted.promise
  assert.equal(await deleteButton.isDisabled(), true)
  deleteRelease.resolve()
  await page.locator('[data-asset-id="binary"]').waitFor({ state: 'hidden' })
  assert.equal(deletes, 1)
  await page.getByRole('button', { name: 'New link', exact: true }).click()
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Draft link')
  await dialog.getByRole('textbox', { name: 'URL', exact: true }).fill('https://example.com/docs')
  const postStarted = deferred(),
    postRelease = deferred()
  let posts = 0
  await page.route('**/api/assets', async (route) => {
    posts++
    postStarted.resolve()
    await postRelease.promise
    return route.fulfill({ status: 503, json: { error: 'Synthetic create failure' } })
  })
  await dialog.locator('form').evaluate((form) => {
    form.requestSubmit()
    form.requestSubmit()
  })
  await postStarted.promise
  await page.locator('[data-slot=dialog-overlay]').click({ position: { x: 5, y: 5 } })
  assert.equal(await dialog.isVisible(), true)
  await page.keyboard.press('Escape')
  assert.equal(await dialog.isVisible(), true)
  assert.equal(
    await dialog.getByRole('button', { name: 'Close dialog', exact: true }).isDisabled(),
    true,
  )
  assert.equal(await dialog.getByRole('button', { name: 'Cancel', exact: true }).isDisabled(), true)
  postRelease.resolve()
  await dialog.getByRole('alert').getByText('Synthetic create failure', { exact: true }).waitFor()
  assert.equal(posts, 1)
  assert.equal(
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).inputValue(),
    'Draft link',
  )
  assert.equal(
    await dialog.getByRole('textbox', { name: 'URL', exact: true }).inputValue(),
    'https://example.com/docs',
  )
  await page.keyboard.press('Escape')
  await dialog.waitFor({ state: 'hidden' })
  await page.unroute('**/api/assets')
  let acceptedPosts = 0
  await page.route('**/api/assets', (route) => {
    acceptedPosts++
    listFailure = true
    return route.fulfill({
      status: 201,
      json: asset('created', 'Created link', {
        kind: 'link',
        url: 'https://example.com/docs',
        mimeType: 'text/uri-list',
      }),
    })
  })
  await page.getByRole('button', { name: 'New link', exact: true }).click()
  await dialog.getByRole('textbox', { name: 'URL', exact: true }).fill('https://example.com/docs')
  await dialog.getByRole('button', { name: 'Add link', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  await page.getByRole('alert').getByText('Synthetic list failure', { exact: true }).waitFor()
  listFailure = false
  await page.getByRole('button', { name: 'Retry', exact: true }).click()
  await page.getByRole('alert').waitFor({ state: 'hidden' })
  assert.equal(acceptedPosts, 1)
  assert.equal(
    await page.evaluate(
      () => window.assetEvidence.notices.filter((message) => message === 'Link asset added').length,
    ),
    1,
  )
  // 同一真实模板验证独立结果滚动、临界宽度与低高度 Dialog footer。
  for (const [width, height, language, large, dark] of [
    [1280, 900, 'en-US', false, false],
    [759, 700, 'en-US', false, false],
    [761, 700, 'en-US', true, false],
    [390, 700, 'zh-CN', true, true],
    [1280, 420, 'en-US', false, true],
  ]) {
    await page.setViewportSize({ width, height })
    await page.evaluate(
      ({ large, dark }) => {
        window.assetFixture.appearance(large, dark)
      },
      { large, dark },
    )
    if (
      (await page.getByRole('button', { name: 'Filters', exact: true }).count()) !==
      (language === 'en-US' ? 1 : 0)
    )
      await page.getByRole('button', { name: 'Language', exact: true }).click()
    const results = page.locator('[data-assets-results]')
    const before = await page.locator('[data-slot="page-toolbar"]').boundingBox()
    await results.evaluate((element) => {
      element.scrollTop = element.scrollHeight
    })
    const after = await page.locator('[data-slot="page-toolbar"]').boundingBox()
    assert.equal(before.y, after.y)
    const geometry = await results.evaluate((element) => ({
      width: element.clientWidth,
      scrollWidth: element.scrollWidth,
      height: element.clientHeight,
      scrollHeight: element.scrollHeight,
      top: element.scrollTop,
    }))
    assert.ok(geometry.scrollWidth <= geometry.width + 1)
    assert.ok(geometry.scrollHeight > geometry.height && geometry.top > 0)
    const long = page.locator('[data-asset-id="long"]')
    const use = long.getByRole('button', {
      name: language === 'en-US' ? 'Use in chat' : '用于对话',
      exact: true,
    })
    await use.scrollIntoViewIfNeeded()
    assert.equal(await use.isVisible(), true)
    await page.screenshot({
      path: join(output, `assets-${width}-${height}-${language}.png`),
      animations: 'disabled',
    })
    await page
      .getByRole('button', {
        name: language === 'en-US' ? 'Preview: Long plain' : '预览: Long plain',
        exact: true,
      })
      .click()
    await dialog.locator('[data-asset-plain]').waitFor()
    await dialog.evaluate(async (element) => {
      await Promise.all(
        element
          .getAnimations({ subtree: true })
          .filter((animation) => animation.effect.getTiming().iterations !== Infinity)
          .map((animation) => animation.finished.catch(() => {})),
      )
    })
    const footer = await dialog.locator('[data-slot="dialog-footer"]').boundingBox()
    assert.ok(footer.y >= 0 && footer.y + footer.height <= height + 1)
    const body = await dialog.locator('[data-slot="dialog-body"]').evaluate((element) => ({
      height: element.clientHeight,
      scrollHeight: element.scrollHeight,
      overflow: getComputedStyle(element).overflowY,
    }))
    assert.ok(body.height > 0 && body.scrollHeight > body.height)
    assert.equal(body.overflow, 'auto')
    await page.screenshot({
      path: join(output, `preview-${width}-${height}-${language}.png`),
      animations: 'disabled',
    })
    const appearance = await page.evaluate(() => ({
      theme: document.documentElement.dataset.theme,
      rootFont: getComputedStyle(document.documentElement).fontSize,
      uiFont: getComputedStyle(document.documentElement).getPropertyValue('--ui-font-size').trim(),
      surface: getComputedStyle(document.documentElement).getPropertyValue('--solid').trim(),
    }))
    assert.equal(appearance.theme, dark ? 'dark' : 'light')
    assert.equal(appearance.rootFont, '16px')
    assert.equal(appearance.uiFont, large ? '16px' : '14px')
    metrics.push({ width, height, language, large, dark, appearance, geometry, footer, body })
    await page.keyboard.press('Escape')
    await dialog.waitFor({ state: 'hidden' })
  }
  listEmpty = true
  await page.getByRole('button', { name: 'Refresh assets', exact: true }).click()
  await page.getByRole('heading', { name: 'Nothing has gathered here yet', exact: true }).waitFor()
  await page.getByRole('button', { name: 'Add link', exact: true }).waitFor()
  await writeFile(join(output, 'metrics.json'), JSON.stringify(metrics, null, 2))
  console.log(
    JSON.stringify({
      output,
      checks: [
        'obsolete query cannot replace current results',
        'filter menu remains open with independent removable conditions',
        'preview opens while loading, cancels on close and restores focus',
        'plain/markdown/binary preview semantics',
        'link duplicate submit and busy close protection; failure retains draft',
        'first-error/no-match/refresh-error distinguish and retain data',
        'current scope and action reads cannot leak across sessions',
        'image retry and per-asset mutation gates',
        'desktop/759/761/390/low-height, two languages, large type and dark theme',
      ],
      errors,
    }),
  )
  assert.deepEqual(errors, [])
} finally {
  await browser?.close()
  await server.close()
  await writeFile(join(output, 'errors.json'), JSON.stringify(errors, null, 2))
}
