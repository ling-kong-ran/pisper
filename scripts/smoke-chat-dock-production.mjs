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
  const makePage = async (width, height = 960, nativeTitlebar = false) => {
    const context = await browser.newContext({ viewport: { width, height }, locale: 'zh-CN' })
    await context.addCookies([{ name: '__pisper_desktop', value: token, url: base }])
    await context.addInitScript((nativeTitlebar) => {
      localStorage.setItem('pisper-language', 'zh-CN')
      localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
      // 仅提供受控的桌面桥接能力；本检查不宣称操作过原生窗口。
      if (nativeTitlebar)
        window.pisperDesktop = {
          platform: 'win32',
          customTitlebar: true,
          windowAction: () => Promise.resolve(),
          terminalProfiles: () => Promise.resolve([]),
          getAppInfo: () =>
            Promise.resolve({
              desktop: true,
              packaged: false,
              version: 'fixture',
              platform: 'win32',
              arch: 'x64',
            }),
          setLanguage: () => Promise.resolve(),
        }
    }, nativeTitlebar)
    await context.route('**/*', (route) => {
      const url = new URL(route.request().url())
      if (url.origin !== base) return route.abort()
      if (url.pathname === '/toolbar-preview-fixture.html')
        return route.fulfill({
          contentType: 'text/html',
          body: '<!doctype html><title>Preview</title><p>Toolbar preview fixture</p>',
        })
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
            title: node
              .closest('.dv-groupview')
              ?.querySelector('[role="tab"][aria-selected="true"]')?.textContent,
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
    assert.equal(await page.getByRole('button', { name: '会话操作', exact: true }).count(), 0)
    await page.getByRole('tab', { name: 'Dock Beta', exact: true }).click({ button: 'right' })
    const save = waitForSave(
      page,
      sessions.slice(0, 2).map(({ id }) => id),
    )
    await page.getByRole('menuitem', { name: '拆分到右侧', exact: true }).click()
    await expectPanels(page, 2)
    await save
    await expectComposerBounds(page)
    const panelFor = (name) =>
      page
        .locator('.dv-groupview')
        .filter({ has: page.getByRole('tab', { name, exact: true }) })
        .locator('.session-dock-panel')
    const alpha = panelFor('Dock Alpha')
    const beta = panelFor('Dock Beta')
    assert.equal(await page.locator('.workbench-chat-header').count(), 0)
    assert.equal(await page.locator('.focus-composer-shell .header-workspace').count(), 2)
    assert.equal(
      await page
        .locator('.dv-tabs-and-actions-container')
        .getByRole('button', { name: /会话上下文/ })
        .count(),
      2,
    )
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
    await phone.goto(base + '/#/assets')
    await phone.getByRole('tab', { name: '工作区', exact: true, selected: true }).waitFor()
    await phone.goto(base + '/#/config/models')
    await phone.getByRole('tab', { name: '对话', exact: true, selected: true }).waitFor()
    await phone.getByRole('heading', { name: '设置', exact: true }).waitFor()
    await phone.context().close()
    await verifyChatDockCreation({ page, sessions })
    const native = await makePage(1440, 960, true)
    await native.goto(base + '/#/chat')
    await native
      .locator('.dockview-theme-pisper')
      .waitFor()
      .catch(async (error) => {
        console.error(
          'Native titlebar fixture startup',
          errors,
          (await native.locator('body').innerText()).slice(0, 1200),
        )
        throw error
      })
    await native.locator('button[title^="Dock Alpha ·"]').click()
    const tools = native
      .locator('.desktop-window-controls')
      .getByRole('button', { name: /会话上下文/ })
    await tools.waitFor()
    // 通过既有公开事件打开分屏网页；切换预览焦点不能移除仍可见会话的工具栏。
    await native.evaluate((url) => {
      window.dispatchEvent(new CustomEvent('pisper:web-preview-open', { detail: { url } }))
    }, base + '/toolbar-preview-fixture.html')
    await native
      .locator('.dv-active-group')
      .getByRole('tab', { name: '127.0.0.1', exact: true, selected: true })
      .waitFor()
    await tools.waitFor()
    assert.equal(await tools.count(), 1)
    await tools.click()
    await native.locator('[data-session-context-panel]').waitFor()
    assert.equal(await tools.count(), 1)
    await native.context().close()
    assert.deepEqual(errors, [])
  } finally {
    await browser.close()
  }
}
