import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'
import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { chromium } from 'playwright-core'
import { SMOKE_BROWSER } from './fixtures/smoke-browser.mjs'

export async function verifyReplicaControls() {
  const output = await mkdtemp(join(tmpdir(), 'pisper-replica-controls-'))
  const root = fileURLToPath(new URL('../', import.meta.url))
  const server = await createServer({
    configFile: false,
    // 独立的依赖预构建缓存：不与仓库共享的 node_modules/.vite（同时被开发服务器使用）互相改写，
    // 避免“vite config has changed”触发的重新预构建在页面使用中途整页重载。
    cacheDir: join(output, 'vite-cache'),
    root,
    plugins: [
      react(),
      tailwindcss(),
      {
        name: 'isolated-replica-controls',
        configureServer(server) {
          server.middlewares.use(async (request, response, next) => {
            const requestPath = new URL(request.url, 'http://fixture').pathname
            if (requestPath === '/__message-review') {
              response.setHeader('Content-Type', 'text/html')
              response.end(
                await server.transformIndexHtml(
                  '/__message-review',
                  '<!doctype html><html lang="zh-CN"><head><link rel="stylesheet" href="/src/index.css"><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><div id="root"></div><script type="module" src="/scripts/fixtures/frontend-review.tsx"></script></body></html>',
                ),
              )
              return
            }
            if (
              requestPath === '/api/sessions/a/tree' ||
              requestPath === '/api/settings/browser-preferences'
            ) {
              response.setHeader('Content-Type', 'application/json')
              response.end(
                JSON.stringify(
                  requestPath.endsWith('/tree') ? { nodes: [], leafId: null } : { preferences: {} },
                ),
              )
              return
            }
            if (request.url?.startsWith('/api/')) {
              response.writeHead(500)
              response.end('No runtime data in control fixture')
              return
            }
            if (request.url !== '/__controls') return next()
            response.setHeader('Content-Type', 'text/html')
            response.end(
              await server.transformIndexHtml(
                '/__controls',
                '<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><div id="root"></div><script type="module" src="/scripts/fixtures/replica-controls.tsx"></script></body></html>',
              ),
            )
          })
        },
      },
    ],
    resolve: { alias: { '@': `${root}src`, '@shared': `${root}shared` } },
    server: { host: '127.0.0.1', port: 0, hmr: false, watch: null },
    appType: 'custom',
  })
  let browser
  try {
    await server.listen()
    browser = await chromium.launch({
      headless: true,
      ...(SMOKE_BROWSER ? { executablePath: SMOKE_BROWSER } : {}),
    })
    const page = await browser.newPage({ viewport: { width: 900, height: 560 } })
    page.setDefaultTimeout(5000)
    const errors = []
    page.on('pageerror', (error) => errors.push(error.message))
    await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/__controls`)
    const ordinary = page.getByTestId('ordinary')
    await ordinary.waitFor()
    await ordinary.click()
    assert.equal(await page.getByTestId('submits').textContent(), '0')
    await page.getByRole('button', { name: 'Submit', exact: true }).click()
    assert.equal(await page.getByTestId('submits').textContent(), '1')

    const stable = page.getByTestId('stable')
    const initial = await stable.boundingBox()
    await page.evaluate(() => {
      window.replicaControls.setLabel('Saving…')
      window.replicaControls.setLoading(true)
    })
    await page.waitForFunction(
      () => document.querySelector('[data-testid="stable"]').getAttribute('aria-busy') === 'true',
    )
    assert.match(
      await stable.ariaSnapshot(),
      /button "Saving…"/,
      'loading keeps its accessible name',
    )
    const saving = await stable.boundingBox()
    assert.ok(Math.abs(initial.width - saving.width) < 1, 'loading does not move adjacent controls')
    await page.evaluate(() => {
      window.replicaControls.setLabel('Saved')
      window.replicaControls.setLoading(false)
    })
    await page.waitForFunction(() =>
      document.querySelector('[data-testid="stable"]').textContent.includes('Saved'),
    )
    assert.ok(Math.abs(initial.width - (await stable.boundingBox()).width) < 1)

    const link = page.getByTestId('disabled-link')
    assert.equal(await link.getAttribute('href'), null)
    for (const name of ['click', 'auxclick'])
      await link.dispatchEvent(name, { button: name === 'auxclick' ? 1 : 0 })
    for (const key of ['Enter', ' ']) await link.dispatchEvent('keydown', { key })
    const explain = page.getByTestId('explain')
    await explain.focus()
    await explain.press('Enter')
    await explain.press('Space')
    assert.equal(await page.getByTestId('activations').textContent(), '0')
    assert.equal(await explain.evaluate((el) => el === document.activeElement), true)

    await page
      .getByTestId('button-states')
      .screenshot({ animations: 'disabled', path: join(output, 'button-states.png') })
    await page.getByTestId('state-normal').hover()
    await page
      .getByTestId('button-states')
      .screenshot({ animations: 'disabled', path: join(output, 'button-hover.png') })
    await page.mouse.down()
    await page
      .getByTestId('button-states')
      .screenshot({ animations: 'disabled', path: join(output, 'button-pressed.png') })
    await page.mouse.up()

    await page.getByTestId('menu-trigger').click()
    const menu = page.getByRole('menu')
    await menu.waitFor()
    assert.equal(
      await page.evaluate(() => document.body.getAttribute('data-scroll-locked')),
      null,
      'nonmodal menus preserve outside scrolling',
    )
    assert.ok(
      (await menu.boundingBox()).width >
        (await page.getByTestId('menu-trigger').boundingBox()).width + 40,
    )
    await page.getByRole('menuitemcheckbox').click()
    await page.waitForFunction(
      () =>
        document
          .querySelector('[role=menuitemcheckbox][aria-checked=true]')
          ?.closest('[data-slot=dropdown-menu-content]')
          ?.getAttribute('data-state') === 'open',
    )
    await page.keyboard.press('End')
    await page.waitForFunction(() => document.activeElement?.textContent === 'Beta action')
    await page.keyboard.press('Home')
    await page.waitForFunction(
      () => document.activeElement?.getAttribute('role') === 'menuitemcheckbox',
    )
    await page.keyboard.type('b')
    await page.waitForFunction(() => document.activeElement?.textContent === 'Beta action')
    await page.keyboard.press('Escape')
    await menu.waitFor({ state: 'hidden' })
    assert.equal(
      await page.getByTestId('menu-trigger').evaluate((el) => el === document.activeElement),
      true,
    )

    await page.getByTestId('menu-trigger').click()
    await menu.waitFor()
    await ordinary.click()
    await menu.waitFor({ state: 'hidden' })
    assert.equal(await page.getByTestId('submits').textContent(), '1')
    assert.equal(
      await page.evaluate(
        () =>
          window.replicaControls.menuTrigger ===
          document.querySelector('[data-testid=menu-trigger]'),
      ),
      true,
      'asChild forwards the trigger ref',
    )
    await page.getByTestId('menu-trigger').click()
    await menu.waitFor()
    await page.getByTestId('outside-input').click()
    await menu.waitFor({ state: 'hidden' })
    assert.equal(
      await page
        .getByTestId('outside-input')
        .evaluate((element) => element === document.activeElement),
      true,
      'outside input keeps focus',
    )
    await page.getByTestId('custom-focus-menu').click()
    await page.getByRole('menuitem', { name: 'Return to input' }).click()
    await menu.waitFor({ state: 'hidden' })
    assert.equal(
      await page
        .getByTestId('outside-input')
        .evaluate((element) => element === document.activeElement),
      true,
      'caller close autofocus remains authoritative',
    )
    await page.getByTestId('menu-trigger').click()
    await page.getByRole('menuitem', { name: 'Alpha action', exact: true }).click()
    await page.getByTestId('menu-trigger').click()
    await page.getByRole('menuitem', { name: 'Beta action', exact: true }).click()
    for (const options of [{ button: 'right' }, { modifiers: ['Control'] }]) {
      await page.getByTestId('menu-trigger').click()
      await page.getByRole('menu').waitFor()
      await page.getByTestId('menu-trigger').click(options)
      await page.getByRole('menu').waitFor({ state: 'hidden' })
      assert.equal(await page.getByTestId('menu-trigger').getAttribute('aria-expanded'), 'false')
    }
    await menu.waitFor({ state: 'hidden' })
    await page.getByTestId('matched-menu-trigger').click()
    await menu.waitFor()
    await menu.evaluate((element) =>
      Promise.all(element.getAnimations().map((animation) => animation.finished)),
    )
    assert.equal(await page.evaluate(() => document.body.getAttribute('data-scroll-locked')), '1')
    assert.ok(
      Math.abs(
        (await menu.boundingBox()).width -
          (await page.getByTestId('matched-menu-trigger').boundingBox()).width,
      ) < 1,
    )
    await page.getByRole('menuitemcheckbox', { name: 'Close after checking' }).click()
    await menu.waitFor({ state: 'hidden' })

    // 会话页真实页头的标题 ▾ 菜单（A6 起取代旧顶栏 TaskWorkspaceHeader，同一个 id task-<id>-more）：
    // “重命名”执行一次并关上菜单，焦点回到标题键。主题切换不在会话菜单里了（规格 2.5：⌘K“切换主题”与设置 › 界面），
    // 跟随系统的循环由 smoke-pisper-ui 经 ⌘K“切换主题”覆盖。
    const workspaceMore = page.locator('#task-control-probe-more')
    await workspaceMore.click()
    await menu.waitFor()
    assert.equal(await workspaceMore.getAttribute('aria-expanded'), 'true')
    await page.getByRole('menuitem', { name: 'Rename chat', exact: true }).click()
    await menu.waitFor({ state: 'hidden' })
    await page.waitForFunction(
      () => document.activeElement?.id === 'task-control-probe-more',
      undefined,
      { timeout: 2000 },
    )
    assert.equal(await page.getByTestId('renames').textContent(), '1')
    await page.getByTestId('dialog-trigger').click()
    const dialog = page.getByRole('dialog', { name: 'Nested controls', exact: true })
    await dialog.waitFor()
    assert.equal(
      await page
        .getByTestId('dialog-body')
        .evaluate((element) => element.scrollHeight > element.clientHeight),
      true,
      'long dialog body scrolls independently',
    )
    await dialog.getByRole('button', { name: 'Nested picker' }).click()
    const nested = page.getByRole('textbox', { name: 'Nested value' })
    await nested.fill('Changed')
    assert.equal(
      await nested.evaluate((el) => Boolean(el.closest('[data-slot="dialog-content"]'))),
      true,
      'nested picker portal belongs to dialog',
    )
    await nested.press('Escape')
    await nested.waitFor({ state: 'hidden' })
    assert.equal(await dialog.isVisible(), true)
    const footer = dialog.getByRole('button', { name: 'Footer action' })
    const footerBounds = await footer.boundingBox()
    assert.ok(
      footerBounds.y >= 0 && footerBounds.y + footerBounds.height <= 560,
      'footer remains reachable at low height',
    )
    await page.evaluate(() => window.replicaControls.setBusy(true))
    await page.waitForFunction(
      () =>
        document.querySelector('[data-slot=dialog-content]')?.getAttribute('aria-busy') === 'true',
    )
    await page.keyboard.press('Escape')
    assert.equal(await dialog.isVisible(), true, 'busy prevents Escape close')
    await dialog.getByRole('button', { name: 'Close dialog', exact: true }).dispatchEvent('click')
    assert.equal(await dialog.isVisible(), true, 'busy prevents close button')
    await page.mouse.click(5, 5)
    assert.equal(await dialog.isVisible(), true, 'busy prevents outside close')
    await page.evaluate(() => window.replicaControls.setBusy(false))
    await page.waitForFunction(
      () => !document.querySelector('[data-slot=dialog-content]')?.hasAttribute('aria-busy'),
    )
    await page.keyboard.press('Escape')
    await dialog.waitFor({ state: 'hidden' })

    await page.evaluate(() => window.replicaControls.openApp())
    const app = page.getByRole('dialog', { name: 'Save fixture', exact: true })
    await app.getByRole('button', { name: 'Save', exact: true }).click()
    await page.waitForFunction(
      () => document.querySelector('[role="dialog"]').getAttribute('aria-busy') === 'true',
    )
    await app.locator('form').evaluate((form) => form.requestSubmit())
    await page.keyboard.press('Escape')
    assert.equal(await app.isVisible(), true)
    assert.equal(await page.getByTestId('finishes').textContent(), '1')
    await page.evaluate(() => window.replicaControls.finishApp(false))
    await app.getByRole('alert').waitFor()
    assert.equal(await app.getByRole('textbox').inputValue(), 'Draft')
    await app.getByRole('button', { name: 'Cancel', exact: true }).click()
    await app.waitFor({ state: 'hidden' })
    await page.evaluate(() => {
      window.replicaControls.throwOnFinish = true
      window.replicaControls.openApp()
    })
    await app.getByRole('button', { name: 'Save', exact: true }).click()
    await app.getByRole('alert').waitFor()
    assert.equal(await app.getByRole('button', { name: 'Save', exact: true }).isEnabled(), true)
    await page.evaluate(() => {
      window.replicaControls.throwOnFinish = false
    })
    await app.getByRole('button', { name: 'Save', exact: true }).click()
    await page.evaluate(() => window.replicaControls.finishApp(true))
    await app.waitFor({ state: 'hidden' })
    assert.equal(await page.getByTestId('finishes').textContent(), '3')

    await page.getByTestId('tooltip-trigger').hover()
    await page.getByRole('tooltip').waitFor()
    assert.equal(
      await page.locator('[data-slot=tooltip-content]').getAttribute('data-state'),
      'delayed-open',
    )
    await page.keyboard.press('Escape')
    await page.getByRole('tooltip').waitFor({ state: 'hidden' })
    await page.getByTestId('tooltip-trigger').focus()
    await page.getByRole('tooltip').waitFor({ timeout: 1000 })
    await page.keyboard.press('Escape')
    await page.getByRole('tooltip').waitFor({ state: 'hidden' })
    await page.setViewportSize({ width: 320, height: 400 })
    await page.getByRole('button', { name: 'Edge picker', exact: true }).click()
    const edge = page.getByText('Viewport constrained content', { exact: true })
    await edge.waitFor()
    const edgeBounds = await edge.locator('..').boundingBox()
    assert.ok(
      edgeBounds.x >= 19 &&
        edgeBounds.y >= 19 &&
        edgeBounds.x + edgeBounds.width <= 301 &&
        edgeBounds.y + edgeBounds.height <= 381,
      'popover respects collision padding at narrow viewport edges',
    )
    await page.screenshot({ animations: 'disabled', path: join(output, 'edge-popover.png') })
    await page.keyboard.press('Escape')
    await page.getByTestId('dialog-trigger').click()
    await dialog.waitFor()
    await page.screenshot({ animations: 'disabled', path: join(output, 'dialog-narrow.png') })
    const narrowFooter = await dialog.getByRole('button', { name: 'Footer action' }).boundingBox()
    assert.ok(narrowFooter.y >= 0 && narrowFooter.y + narrowFooter.height <= 400)
    await page.keyboard.press('Escape')
    await dialog.waitFor({ state: 'hidden' })
    await page.evaluate(() => {
      document.documentElement.classList.add('dark')
      document.documentElement.dataset.theme = 'dark'
    })
    await page
      .getByTestId('button-states')
      .screenshot({ animations: 'disabled', path: join(output, 'button-states-dark.png') })
    await page.setViewportSize({ width: 390, height: 844 })
    await page.evaluate(() => window.replicaControls.setMenuMounted(false))
    await page.waitForFunction(() => window.replicaControls.menuTrigger === null)
    await page.goto(
      `http://127.0.0.1:${server.httpServer.address().port}/__message-review?mode=message`,
    )

    const messageMore = page.getByRole('button', { name: '更多回复操作', exact: true })
    await messageMore.press('Enter')
    await page.getByRole('menuitem', { name: '从此处另开对话', exact: true }).click()
    assert.equal(await page.getByLabel('Fixture action').textContent(), 'child:boundary')
    await messageMore.press('Enter')
    await page.getByRole('menuitem', { name: '从此处继续', exact: true }).click()
    assert.equal(await page.getByLabel('Fixture action').textContent(), 'branch:boundary')
    await page.waitForFunction(
      () => document.activeElement?.getAttribute('aria-label') === '更多回复操作',
    )
    assert.deepEqual(errors, [])
    return {
      output,
      assertions: 'button, link, dropdown, nested popover, busy dialog, async AppDialog, tooltip',
    }
  } finally {
    await browser?.close()
    await server.close()
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  console.log(await verifyReplicaControls())
}
