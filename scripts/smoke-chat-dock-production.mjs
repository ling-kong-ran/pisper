// 完整生产入口验收，防止仅测试 Dock 组件而漏掉 ChatPage 不再挂载它的回归。
import assert from 'node:assert/strict'
import { chromium } from 'playwright-core'

export async function verifyProductionChatDock({ base, token, workspace }) {
  const sessions = []
  for (const name of ['Dock Alpha', 'Dock Beta', 'Dock Gamma']) {
    const response = await fetch(base + '/api/sessions', {
      method: 'POST',
      headers: {
        Origin: base,
        Cookie: `__pisper_desktop=${token}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ name, cwd: workspace }),
      signal: AbortSignal.timeout(15000),
    })
    assert.equal(response.status, 201)
    sessions.push(await response.json())
  }
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_SMOKE_BROWSER
      ? { executablePath: process.env.PISPER_SMOKE_BROWSER }
      : {}),
  })
  const errors = []
  const makePage = async (width) => {
    const context = await browser.newContext({ viewport: { width, height: 960 }, locale: 'zh-CN' })
    await context.addCookies([{ name: '__pisper_desktop', value: token, url: base }])
    await context.addInitScript(() => {
      localStorage.setItem('pisper-language', 'zh-CN')
      localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
    })
    await context.route('**/*', (route) => {
      const url = new URL(route.request().url())
      if (url.origin !== base) return route.abort()
      if (url.pathname.startsWith('/api/app-update')) {
        return route.fulfill({ json: { state: 'current', canDownload: false } })
      }
      return route.continue()
    })
    const page = await context.newPage()
    page.setDefaultTimeout(15000)
    page.on('pageerror', (error) => errors.push(error.message))
    return page
  }
  const expectPanels = (page, count) =>
    page.waitForFunction((expected) => {
      const visible = [...document.querySelectorAll('.session-dock-panel')].filter(
        (node) =>
          node.checkVisibility({ visibilityProperty: true }) &&
          node.getBoundingClientRect().height > 0,
      )
      return visible.length === expected
    }, count)
  const waitForSave = (page, ids) =>
    page.waitForResponse((response) => {
      const request = response.request()
      if (!response.url().endsWith('/api/settings/chat-dock-layout') || request.method() !== 'PUT')
        return false
      const panels = request.postDataJSON()?.layout?.panels
      return response.ok() && ids.every((id) => panels?.[`session:${id}`])
    })
  try {
    const page = await makePage(1440)
    await page.goto(base + '/#/chat')
    await page.locator('.dockview-theme-pisper').waitFor()
    await page.locator('button[title^="Dock Alpha ·"]').click()
    await page.locator('button[title^="Dock Beta ·"]').click()
    await page.getByRole('button', { name: '会话操作', exact: true }).click()
    const save = waitForSave(
      page,
      sessions.slice(0, 2).map(({ id }) => id),
    )
    await page.getByRole('menuitem', { name: '拆分到右侧', exact: true }).click()
    await expectPanels(page, 2)
    await save
    const alpha = page.locator('.session-dock-panel').filter({ hasText: 'Dock Alpha' })
    const beta = page.locator('.session-dock-panel').filter({ hasText: 'Dock Beta' })
    await alpha.getByRole('textbox', { name: '任务描述', exact: true }).fill('Alpha draft')
    await beta.getByRole('textbox', { name: '任务描述', exact: true }).fill('Beta draft')
    assert.equal(
      await alpha.getByRole('textbox', { name: '任务描述', exact: true }).inputValue(),
      'Alpha draft',
    )
    assert.equal(
      await beta.getByRole('textbox', { name: '任务描述', exact: true }).inputValue(),
      'Beta draft',
    )
    await page.reload()
    await expectPanels(page, 2)

    // 从真正的历史页进入新分屏，覆盖跨路由的选择请求重放。
    await page.evaluate(() => {
      location.hash = '#/chat/history'
    })
    await page.getByRole('button', { name: 'Dock Gamma 的更多操作', exact: true }).click()
    const saveThird = waitForSave(
      page,
      sessions.map(({ id }) => id),
    )
    await page.getByRole('menuitem', { name: '拆分到下方', exact: true }).click()
    await expectPanels(page, 3)
    await saveThird

    // 全新手机页面必须走完整 ChatPage 单会话入口，不能请求桌面分屏 chunk。
    const phone = await makePage(390)
    const dockRequests = []
    phone.on('request', (request) => {
      if (/\/(?:ChatDockView|vendor-dockview)-[^/]+\.js/.test(request.url()))
        dockRequests.push(request.url())
    })
    await phone.goto(base + '/#/chat')
    await expectPanels(phone, 1)
    assert.equal(await phone.locator('.dockview-theme-pisper').count(), 0)
    assert.deepEqual(dockRequests, [])
    assert.deepEqual(errors, [])
  } finally {
    await browser.close()
  }
}
