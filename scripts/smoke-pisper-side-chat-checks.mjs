import assert from 'node:assert/strict'
import { join } from 'node:path'

// 只使用调用方创建的隔离 Runtime 和本地模型；不连接用户正在运行的 dev 服务。
export async function verifySideChat({ page, base, api, report, output, parentSessionId }) {
  const endpoint = new URL(base)
  assert.equal(endpoint.protocol, 'http:')
  assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(endpoint.hostname))
  assert.equal(new URL(page.url()).origin, endpoint.origin)
  const activeSession = () => page.evaluate(() => localStorage.getItem('pisper-active-session'))
  assert.equal(await activeSession(), parentSessionId)

  const viewport = page.viewportSize()
  // 手机抽屉打开时主输入框会被 aria-hidden，仍须检查其草稿没有被侧聊覆盖。
  const mainInput = page.locator('textarea[aria-label="任务描述"]')
  const mainDraft = await mainInput.inputValue()
  const mainDraftMarker = 'PI parent draft stays separate from temporary chat'
  const panel = page.locator(`[data-side-chat-parent="${parentSessionId}"]`)
  const input = panel.getByRole('textbox', { name: '临时聊天消息', exact: true })
  const send = panel.getByRole('button', { name: '发送临时消息', exact: true })
  const stop = panel.getByRole('button', { name: '停止临时回复', exact: true })
  const closeTab = page.getByRole('button', { name: '关闭辅助页面 · 临时聊天', exact: true })
  const contextWasOpen = await page
    .getByRole('button', { name: '关闭会话上下文', exact: true })
    .first()
    .isVisible()
  const selectedTab = page.getByRole('tab', { selected: true }).first()
  const previousTab = (await selectedTab.count()) ? await selectedTab.textContent() : null
  const sidePath = `/api/sessions/${parentSessionId}/side-chat`
  const catalogIds = (catalog) => catalog.sessions.map((session) => session.id).sort()
  const catalogBefore = await api('/api/sessions')
  const parentBefore = catalogBefore.sessions.find((session) => session.id === parentSessionId)
  assert.ok(parentBefore, 'The caller must provide an existing ordinary parent session')
  const historyBefore = await api(`/api/sessions/${parentSessionId}/messages?limit=50`)
  const sideBefore = await api(sidePath)
  assert.equal(sideBefore.session, null, 'The fixture parent must not already have a side chat')

  let sideId
  let stage = 'open'
  const fixtureRoutes = []
  let restoreBrowserTime
  async function addFixtureRoute(path, handler) {
    const url = base + path
    await page.route(url, handler)
    fixtureRoutes.push({ url, handler })
  }
  async function removeFixtureRoutes() {
    for (const { url, handler } of fixtureRoutes.splice(0)) await page.unroute(url, handler)
  }
  async function openSideChat() {
    const add = page.getByRole('button', { name: '新增辅助页面', exact: true })
    if (!(await add.isVisible()))
      await page.getByRole('button', { name: '打开会话上下文', exact: true }).click()
    await add.click()
    await page.getByRole('menuitem', { name: '临时聊天', exact: true }).click()
    await input.waitFor()
    assert.equal(await page.getByRole('tab', { name: '临时聊天', exact: true }).count(), 1)
  }
  async function assertParentUnchanged() {
    assert.equal(await activeSession(), parentSessionId, 'Side chat must not select its session')
    assert.equal(await mainInput.inputValue(), mainDraftMarker)
    assert.deepEqual(
      await api(`/api/sessions/${parentSessionId}/messages?limit=50`),
      historyBefore,
      'Temporary messages must not append to or rewrite the parent history',
    )
    assert.deepEqual(catalogIds(await api('/api/sessions')), catalogIds(catalogBefore))
  }
  async function assertInputFits() {
    await input.scrollIntoViewIfNeeded()
    const bounds = await input.boundingBox()
    const view = page.viewportSize()
    assert.ok(bounds && bounds.width > 120 && bounds.height > 24)
    assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= view.width + 1)
    assert.ok(bounds.y >= 0 && bounds.y + bounds.height <= view.height + 1)
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth),
      false,
      'Temporary chat must not cause horizontal page overflow',
    )
    assert.ok(
      await panel.evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
      'Temporary chat controls must fit inside their panel',
    )
  }
  try {
    await page.setViewportSize({ width: 1440, height: 900 })
    await mainInput.fill(mainDraftMarker)
    await openSideChat()
    await openSideChat()
    assert.equal((await api(sidePath)).session, null)
    await assertParentUnchanged()
    assert.equal(
      await panel.getByRole('button', { name: /^模型与智力 ·|^执行模式 ·|^审批模式：/ }).count(),
      0,
      'Temporary chat stays compact without duplicating the parent toolbar',
    )
    report.checks.push(
      'Temporary chat opens from the auxiliary-page menu without creating a session; repeated opening selects its single tab and keeps the parent draft active',
    )

    stage = 'send'
    const firstMessage = 'side-chat isolated first message [pi-ui-sse]'
    await input.fill(firstMessage)
    const creation = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === sidePath && response.request().method() === 'POST',
    )
    await send.click()
    const response = await creation
    assert.equal(response.ok(), true)
    const created = await response.json()
    assert.equal(created.created, true)
    sideId = created.session?.id
    assert.ok(sideId && sideId !== parentSessionId)
    for (const key of [
      'cwd',
      'model',
      'thinkingLevel',
      'executionMode',
      'permissionMode',
      'runMode',
    ]) {
      assert.equal(typeof parentBefore[key], 'string', `Fixture parent must expose ${key}`)
      assert.equal(created.session[key], parentBefore[key], `Temporary chat inherits ${key}`)
    }
    const remaining = Date.parse(created.expiresAt) - Date.now()
    assert.ok(remaining > 23 * 60 * 60 * 1000 && remaining <= 24 * 60 * 60 * 1000 + 60000)
    await panel.getByText('流式回复：验收通过', { exact: true }).waitFor()
    await send.waitFor()
    const ownHistory = await api(`/api/sessions/${sideId}/messages?limit=50`)
    assert.match(JSON.stringify(ownHistory), /side-chat isolated first message/)
    assert.match(JSON.stringify(ownHistory), /流式回复：验收通过/)
    await assertParentUnchanged()
    assert.equal((await api(sidePath)).session.id, sideId)
    await page.screenshot({ path: join(output, 'side-chat-desktop.png'), animations: 'disabled' })
    report.checks.push(
      'First temporary send creates a hidden session inheriting directory, model, thinking, execution, permission and run mode; real local SSE persists only its own messages with a 24-hour expiry',
    )

    stage = 'restore'
    const sideDraft = 'PI temporary draft survives closing the tab'
    await input.fill(sideDraft)
    await closeTab.click()
    await panel.waitFor({ state: 'hidden' })
    await openSideChat()
    await panel.getByText('流式回复：验收通过', { exact: true }).waitFor()
    assert.equal(await input.inputValue(), sideDraft)
    assert.equal((await api(sidePath)).session.id, sideId)
    await assertParentUnchanged()
    report.checks.push(
      'Closing and reopening the temporary tab restores the same history and draft',
    )

    stage = 'mobile-stop'
    await page.setViewportSize({ width: 390, height: 844 })
    // 已打开的面板在手机上继续以抽屉显示；用户关闭后仍可手动重开。
    await page.locator('[data-slot="sheet-content"]').waitFor()
    await page.keyboard.press('Escape')
    await panel.waitFor({ state: 'hidden' })
    await openSideChat()
    await assertInputFits()
    await input.fill('side-chat stop-test [pi-ui-sse]')
    await input.press('Shift+Enter')
    assert.ok((await input.inputValue()).includes('\n'))
    await send.click()
    await stop.waitFor()
    await panel
      .getByText(/正在生成停止测试/)
      .first()
      .waitFor()
    await page.screenshot({ path: join(output, 'side-chat-mobile.png'), animations: 'disabled' })
    // 关闭只隐藏界面；后端继续执行，重新打开后仍可停止该临时会话。
    await closeTab.click()
    await panel.waitFor({ state: 'hidden' })
    await openSideChat()
    await stop.waitFor()
    const stopped = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === `/api/sessions/${sideId}/abort` &&
        response.request().method() === 'POST',
    )
    await stop.click()
    assert.equal((await stopped).ok(), true)
    await send.waitFor()
    await assertInputFits()
    await assertParentUnchanged()
    report.checks.push(
      'At 390px temporary input supports multiline text without overflow; closing during generation retains the run and reopening Stop targets only the temporary session',
    )

    stage = 'approval'
    const approvalId = 'side-chat-ui-approval'
    let approvalPending = true
    const resolutions = []
    // 模型夹具只输出文本；仅补充 live 审批投影来检查 UI 把允许动作交给正确会话。
    await addFixtureRoute(`/api/sessions/${sideId}/live`, async (route) => {
      const response = await route.fetch()
      assert.equal(response.ok(), true)
      const snapshot = await response.json()
      await route.fulfill({
        response,
        json: {
          ...snapshot,
          approvals: approvalPending
            ? [
                {
                  id: approvalId,
                  toolName: 'write',
                  reason: 'Temporary chat UI fixture approval',
                  args: { path: 'side-chat-fixture.txt', content: 'UI fixture only' },
                },
              ]
            : [],
        },
      })
    })
    await addFixtureRoute(`/api/sessions/${sideId}/approvals/${approvalId}`, async (route) => {
      assert.equal(route.request().method(), 'POST')
      resolutions.push(route.request().postDataJSON())
      approvalPending = false
      await route.fulfill({ json: { found: true, approved: true, alreadyResolved: false } })
    })
    await closeTab.click()
    await openSideChat()
    const approval = panel.locator(`[data-pisper-approval-id="${approvalId}"]`)
    const approvedResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === `/api/sessions/${sideId}/approvals/${approvalId}` &&
        response.request().method() === 'POST',
    )
    await approval.getByRole('button', { name: '允许', exact: true }).click()
    assert.equal((await approvedResponse).ok(), true)
    await approval.waitFor({ state: 'hidden' })
    assert.deepEqual(resolutions, [{ approved: true }])
    await removeFixtureRoutes()
    await assertParentUnchanged()
    report.mockedServices.push('temporary-chat approval projection and approval acknowledgment')
    report.checks.push(
      'Temporary tool approval renders inside its own pane and Allow posts only to the temporary session (localized approval UI fixture)',
    )

    stage = 'expiry'
    const expiryDraft = 'PI unsent temporary draft must survive expiration'
    await input.fill(expiryDraft)
    const oldSideId = sideId
    const oldSide = await api(sidePath)
    const browserNow = await page.evaluate(() => Date.now())
    const realNow = Date.now()
    restoreBrowserTime = () => page.clock.setSystemTime(new Date(browserNow + Date.now() - realNow))
    let expirationReads = 0
    let expired = false
    let replacementCreated = false
    await addFixtureRoute(sidePath, async (route) => {
      if (route.request().method() === 'GET') {
        expirationReads++
        if (expirationReads === 1) {
          await route.fulfill({
            json: { ...oldSide, expiresAt: new Date(browserNow + 1000).toISOString() },
          })
        } else if (!replacementCreated) {
          expired = true
          await route.fulfill({ json: { session: null, expiresAt: null, created: false } })
        } else {
          await route.continue()
        }
        return
      }
      assert.equal(route.request().method(), 'POST')
      assert.equal(expired, true)
      // 24 小时的真实清理边界由 Runtime 单测验证；这里只在点击新开后清除本模块创建的
      // 旧测试会话，再交给真实 POST 创建新会话，避免伪造新 ID 或误发用户草稿。
      await api(`/api/sessions/${oldSideId}`, 'DELETE')
      replacementCreated = true
      await route.continue()
    })
    await closeTab.click()
    const nearExpiryResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === sidePath && response.request().method() === 'GET',
    )
    await openSideChat()
    await nearExpiryResponse
    await page.waitForFunction(
      () => document.querySelector('[aria-label="发送临时消息"]')?.disabled === false,
    )
    assert.equal(await input.inputValue(), expiryDraft)
    const expiredResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === sidePath &&
        response.request().method() === 'GET' &&
        expired,
    )
    // 推进页面时钟来触发到期检查，不改变 Runtime 时钟，也不靠实际等待 24 小时。
    await page.clock.fastForward(61_000)
    await expiredResponse
    const renew = panel.getByRole('button', { name: '新开侧边聊天', exact: true })
    await renew.waitFor()
    await panel.getByText(/^临时聊天已过期。/).waitFor()
    assert.equal(await send.isDisabled(), true)
    assert.equal(await input.inputValue(), expiryDraft)
    await page.screenshot({ path: join(output, 'side-chat-expired.png'), animations: 'disabled' })
    const renewedResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === sidePath && response.request().method() === 'POST',
    )
    await renew.click()
    const renewed = await (await renewedResponse).json()
    assert.equal(renewed.created, true)
    assert.ok(renewed.session.id !== oldSideId && renewed.session.id !== parentSessionId)
    sideId = renewed.session.id
    await renew.waitFor({ state: 'hidden' })
    await page.waitForFunction(
      () => document.querySelector('[aria-label="发送临时消息"]')?.disabled === false,
    )
    assert.equal(await input.inputValue(), expiryDraft)
    assert.deepEqual((await api(`/api/sessions/${sideId}/messages?limit=50`)).messages, [])
    await removeFixtureRoutes()
    await restoreBrowserTime()
    restoreBrowserTime = null
    await assertParentUnchanged()
    report.mockedServices.push('temporary-chat expiry response; Runtime TTL verified separately')
    report.checks.push(
      'While the temporary pane stays open, expiration reveals Start new side chat and disables Send; explicit renewal creates a new hidden session while retaining the unsent draft without sending it',
    )
  } catch (error) {
    await page.screenshot({
      path: join(output, `side-chat-failure-${stage}.png`),
      animations: 'disabled',
    })
    throw error
  } finally {
    await removeFixtureRoutes()
    await restoreBrowserTime?.()
    // 不删除传入的父会话；调用方最终关闭 Runtime 并统一清理隔离数据。
    if (await closeTab.isVisible()) await closeTab.click()
    if (viewport) await page.setViewportSize(viewport)
    if (contextWasOpen && previousTab) {
      const tab = page.getByRole('tab', { name: previousTab, exact: true })
      if (await tab.isVisible()) await tab.click()
    } else if (!contextWasOpen) {
      const close = page.getByRole('button', { name: '关闭会话上下文', exact: true }).last()
      if (await close.isVisible()) await close.click()
    }
    if (await mainInput.isVisible()) await mainInput.fill(mainDraft)
  }
}
