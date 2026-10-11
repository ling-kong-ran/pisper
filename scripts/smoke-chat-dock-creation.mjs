// 在真实 ChatPage 中延迟网络响应，覆盖分组新建会话的归属与异步竞态。
import assert from 'node:assert/strict'

function withinTimeout(promise, label) {
  let timer
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${label} did not arrive`)), 15000)
  })
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer))
}

function layoutGroups(node) {
  if (node?.type === 'leaf') return [node.data.views]
  return Array.isArray(node?.data) ? node.data.flatMap(layoutGroups) : []
}

function waitForSavedGroup(page, requiredIds, { absentId } = {}) {
  return page.waitForResponse((response) => {
    const request = response.request()
    if (!response.url().endsWith('/api/settings/chat-dock-layout') || request.method() !== 'PUT')
      return false
    const layout = request.postDataJSON()?.layout
    return (
      response.ok() &&
      (!absentId || !layout?.panels?.[`session:${absentId}`]) &&
      layoutGroups(layout?.grid?.root).some((ids) =>
        requiredIds.every((id) => ids.includes(`session:${id}`)),
      )
    )
  })
}

async function interceptSessionRequests(page, { failLists = false } = {}) {
  const posts = []
  const waitingPosts = []
  const pendingHandlers = new Set()
  const listGate = Promise.withResolvers()
  const firstList = Promise.withResolvers()
  let listsReleased = false
  let closing = false
  const pattern = '**/api/sessions*'
  const handler = (route) => {
    const operation = (async () => {
      const request = route.request()
      if (new URL(request.url()).pathname !== '/api/sessions') return route.fallback()
      if (request.method() === 'POST') {
        // 真正写入隔离 Rust Runtime；只延迟页面收到响应，不能用假 ID 蒙混创建流程。
        const response = await route.fetch()
        assert.equal(response.status(), 201)
        const created = await response.json()
        const gate = Promise.withResolvers()
        const post = { created, requested: request.postDataJSON(), gate }
        posts.push(post)
        waitingPosts.shift()?.resolve(post)
        if (closing) gate.resolve()
        await gate.promise
        return route.fulfill({ response })
      }
      if (request.method() === 'GET') {
        const response = await route.fetch()
        firstList.resolve()
        await listGate.promise
        return failLists
          ? route.fulfill({ status: 503, json: { error: 'fixture catalog unavailable' } })
          : route.fulfill({ response })
      }
      return route.fallback()
    })()
    pendingHandlers.add(operation)
    operation.finally(() => pendingHandlers.delete(operation)).catch(() => {})
    return operation
  }
  await page.route(pattern, handler)
  let consumedPosts = 0
  return {
    nextPost() {
      if (posts[consumedPosts]) return Promise.resolve(posts[consumedPosts++])
      consumedPosts++
      const waiter = Promise.withResolvers()
      waitingPosts.push(waiter)
      return withinTimeout(waiter.promise, 'Session POST')
    },
    async releaseLists() {
      await withinTimeout(firstList.promise, 'Session catalog GET')
      const response = page.waitForResponse((candidate) => {
        const request = candidate.request()
        return new URL(candidate.url()).pathname === '/api/sessions' && request.method() === 'GET'
      })
      listsReleased = true
      listGate.resolve()
      await response
    },
    async close() {
      closing = true
      for (const post of posts) post.gate.resolve()
      if (!listsReleased) listGate.resolve()
      await page.unroute(pattern, handler)
      await Promise.all([...pendingHandlers])
    },
  }
}

const groupWithTab = (page, title) =>
  page.locator('.dv-groupview').filter({ has: page.getByRole('tab', { name: title, exact: true }) })

async function expectTabCount(group, count) {
  await group
    .locator('.dv-tab')
    .nth(count - 1)
    .waitFor()
  assert.equal(await group.locator('.dv-tab').count(), count)
}

async function expectCreating(button) {
  await button
    .page()
    .waitForFunction(
      (element) => element.disabled && element.getAttribute('aria-busy') === 'true',
      await button.elementHandle(),
    )
}

async function expectSelected(page, title) {
  await page
    .locator('.dv-groupview.dv-active-group')
    .getByRole('tab', { name: title, exact: true, selected: true })
    .waitFor()
}

export async function verifyChatDockCreation({ page, sessions }) {
  const [alpha, beta, gamma] = sessions
  await page.setViewportSize({ width: 1440, height: 960 })
  const alphaGroup = groupWithTab(page, alpha.name)
  const betaGroup = groupWithTab(page, beta.name)
  const alphaPlus = alphaGroup.getByRole('button', { name: '新会话', exact: true })
  const betaPlus = betaGroup.getByRole('button', { name: '新会话', exact: true })
  const counts = [
    await alphaGroup.locator('.dv-tab').count(),
    await betaGroup.locator('.dv-tab').count(),
  ]

  // 非活动上组的 + 必须立即打开同组页签，即使目录刷新还挂起或失败。
  await betaGroup.getByRole('tab', { name: beta.name, exact: true }).click()
  await expectSelected(page, beta.name)
  const failedCatalog = await interceptSessionRequests(page, { failLists: true })
  try {
    await alphaPlus.click()
    const post = await failedCatalog.nextPost()
    await expectCreating(alphaPlus)
    assert.equal(post.requested.cwd, alpha.cwd)
    assert.equal(post.created.cwd, alpha.cwd)
    const saved = waitForSavedGroup(page, [alpha.id, post.created.id])
    post.gate.resolve()
    await expectTabCount(alphaGroup, counts[0] + 1)
    assert.equal(await betaGroup.locator('.dv-tab').count(), counts[1])
    await saved
    await betaGroup.getByRole('tab', { name: beta.name, exact: true }).click()
    await failedCatalog.releaseLists()
    await expectSelected(page, beta.name)
    await expectTabCount(alphaGroup, counts[0] + 1)
    assert.equal(await page.locator('.session-dock-missing').count(), 0)
  } finally {
    await failedCatalog.close()
  }

  // 每组独立在途请求；点击另一个组不能复用第一个组的 ID 或 cwd。
  const delayedCatalog = await interceptSessionRequests(page)
  try {
    await alphaPlus.click()
    const alphaPost = await delayedCatalog.nextPost()
    await expectCreating(alphaPlus)
    assert.equal(await betaPlus.isEnabled(), true)
    await betaPlus.click()
    const betaPost = await delayedCatalog.nextPost()
    await expectCreating(betaPlus)
    await expectCreating(alphaPlus)
    assert.notEqual(alphaPost.created.id, betaPost.created.id)
    assert.equal(alphaPost.created.cwd, alpha.cwd)
    assert.equal(betaPost.created.cwd, beta.cwd)
    assert.equal(alphaPost.requested.cwd, alpha.cwd)
    assert.equal(betaPost.requested.cwd, beta.cwd)
    // 第二个请求先返回，确保归属不依赖请求完成顺序或当前活动组。
    const savedBeta = waitForSavedGroup(page, [beta.id, betaPost.created.id])
    betaPost.gate.resolve()
    await expectTabCount(betaGroup, counts[1] + 1)
    await savedBeta
    const savedAlpha = waitForSavedGroup(page, [alpha.id, alphaPost.created.id])
    alphaPost.gate.resolve()
    await expectTabCount(alphaGroup, counts[0] + 2)
    await savedAlpha
    await betaGroup.getByRole('tab', { name: beta.name, exact: true }).click()
    await delayedCatalog.releaseLists()
    await expectSelected(page, beta.name)
    await expectTabCount(alphaGroup, counts[0] + 2)
    await expectTabCount(betaGroup, counts[1] + 1)
    assert.equal(await page.locator('.session-dock-missing').count(), 0)
  } finally {
    await delayedCatalog.close()
  }

  // 创建期间关闭目标组，响应不能再向已经销毁的 Dock 分组移动。
  const closedGroup = await interceptSessionRequests(page)
  try {
    const gammaGroup = groupWithTab(page, gamma.name)
    const groupsBefore = await page.locator('.dv-groupview').count()
    assert.equal(await gammaGroup.locator('.dv-tab').count(), 1)
    await gammaGroup.getByRole('button', { name: '新会话', exact: true }).click()
    const post = await closedGroup.nextPost()
    await gammaGroup
      .getByRole('tab', { name: gamma.name, exact: true })
      .locator('.dv-default-tab-action')
      .click()
    await gammaGroup.waitFor({ state: 'detached' })
    await betaGroup.getByRole('tab', { name: beta.name, exact: true }).click()
    const saved = waitForSavedGroup(page, [beta.id, post.created.id], { absentId: gamma.id })
    post.gate.resolve()
    await expectTabCount(betaGroup, counts[1] + 2)
    await saved
    assert.equal(await page.locator('.dv-groupview').count(), groupsBefore - 1)
    assert.equal(post.created.cwd, gamma.cwd)
    await closedGroup.releaseLists()
    assert.equal(await page.locator('.session-dock-missing').count(), 0)
  } finally {
    await closedGroup.close()
  }
}
