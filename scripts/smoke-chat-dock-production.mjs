// 完整生产入口验收，防止仅测试 Dock 组件而漏掉 ChatPage 不再挂载它的回归。
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { chromium } from 'playwright-core'
import { verifyChatDockCreation } from './smoke-chat-dock-creation.mjs'

export async function verifyProductionChatDock({ base, token, workspace }) {
  const sessions = []
  for (const name of ['Dock Alpha', 'Dock Beta', 'Dock Gamma']) {
    const cwd = join(workspace, name.replace('Dock ', '').toLowerCase())
    await mkdir(cwd)
    const response = await fetch(base + '/api/sessions', {
      method: 'POST',
      headers: {
        Origin: base,
        Cookie: `__pisper_desktop=${token}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ name, cwd }),
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
  const layoutWrites = []
  const makePage = async (width, height = 960) => {
    const context = await browser.newContext({ viewport: { width, height }, locale: 'zh-CN' })
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
    page.on('request', (request) => {
      if (request.url().endsWith('/api/settings/chat-dock-layout') && request.method() === 'PUT') {
        const layout = request.postDataJSON()?.layout
        layoutWrites.push({ panels: Object.keys(layout?.panels || {}), grid: layout?.grid })
      }
    })
    return page
  }
  const expectPanels = async (page, count) => {
    try {
      await page.waitForFunction((expected) => {
        const visible = [...document.querySelectorAll('.session-dock-panel')].filter(
          (node) =>
            node.checkVisibility({ visibilityProperty: true }) &&
            node.getBoundingClientRect().height > 0,
        )
        return visible.length === expected
      }, count)
    } catch (error) {
      console.error(
        'Dock visibility diagnostic',
        await page.evaluate(() => ({
          width: innerWidth,
          height: innerHeight,
          panels: [...document.querySelectorAll('.session-dock-panel')].map((node) => ({
            rect: node.getBoundingClientRect().toJSON(),
            visibility: getComputedStyle(node).visibility,
            title: node.querySelector('[aria-label="重命名会话"]')?.textContent,
          })),
          groups: document.querySelectorAll('.dv-groupview').length,
        })),
        errors,
        JSON.stringify(layoutWrites),
      )
      throw error
    }
  }
  const waitForSave = (page, ids, groups = 2) =>
    page.waitForResponse((response) => {
      const request = response.request()
      if (!response.url().endsWith('/api/settings/chat-dock-layout') || request.method() !== 'PUT')
        return false
      const panels = request.postDataJSON()?.layout?.panels
      const countGroups = (node) =>
        node?.type === 'leaf'
          ? 1
          : Array.isArray(node?.data)
            ? node.data.reduce((count, child) => count + countGroups(child), 0)
            : 0
      return (
        response.ok() &&
        ids.every((id) => panels?.[`session:${id}`]) &&
        countGroups(request.postDataJSON()?.layout?.grid?.root) >= groups
      )
    })
  const expectComposerBounds = async (page) => {
    // 两个面板“可见”不足以证明可用：输入区及按钮还必须完整留在各自分屏内。
    await page.waitForFunction(() =>
      [...document.querySelectorAll('.session-dock-panel')].every((panel) => {
        const bounds = panel.getBoundingClientRect()
        const workspace = panel.closest('.chat-dock-workspace').getBoundingClientRect()
        if (bounds.right > workspace.right + 1 || bounds.bottom > workspace.bottom + 1) return false
        return [
          ...panel.querySelectorAll('.focus-composer textarea, .focus-composer button'),
        ].every((control) => {
          if (!control.checkVisibility({ visibilityProperty: true })) return true
          const rect = control.getBoundingClientRect()
          return (
            rect.left >= bounds.left - 1 &&
            rect.right <= bounds.right + 1 &&
            rect.top >= bounds.top - 1 &&
            rect.bottom <= bounds.bottom + 1
          )
        })
      }),
    )
  }
  try {
    const page = await makePage(825, 712)
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
    await expectComposerBounds(page)
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
    await alpha.getByRole('button', { name: '展开快捷操作', exact: true }).click()
    for (const id of ['permission', 'run-mode', 'model']) {
      const inline = alpha.locator(`.focus-composer-visible-tools [data-composer-tool-id="${id}"]`)
      const overflow = page.locator(`.composer-tool-tray [data-composer-tool-id="${id}"]`)
      assert.ok(
        (await inline.isVisible()) || (await overflow.isVisible()),
        `${id} remains accessible in narrow splits`,
      )
    }
    await page.keyboard.press('Escape')
    await expectPanels(page, 2)
    await page.setViewportSize({ width: 1440, height: 960 })
    await expectPanels(page, 2)
    await expectComposerBounds(page)
    assert.equal(
      await alpha.getByRole('textbox', { name: '任务描述', exact: true }).inputValue(),
      'Alpha draft',
    )
    await page.setViewportSize({ width: 825, height: 712 })
    await expectPanels(page, 2)
    await expectComposerBounds(page)
    await page.reload()
    await expectPanels(page, 2)
    await expectComposerBounds(page)

    // 从真正的历史页进入新分屏，覆盖跨路由的选择请求重放。
    await page.evaluate(() => {
      location.hash = '#/chat/history'
    })
    await page.getByRole('button', { name: 'Dock Gamma 的更多操作', exact: true }).click()
    const saveThird = waitForSave(
      page,
      sessions.map(({ id }) => id),
      3,
    )
    await page.getByRole('menuitem', { name: '拆分到下方', exact: true }).click()
    await expectPanels(page, 3)
    await saveThird
    await expectComposerBounds(page)

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
    await phone.context().close()
    await verifyChatDockCreation({ page, sessions })
    assert.deepEqual(errors, [])
  } finally {
    await browser.close()
  }
}
