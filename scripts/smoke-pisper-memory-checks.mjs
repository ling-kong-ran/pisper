// 真实浏览器 + 隔离 Runtime 走一遍记忆页：只有“抽取到的记忆”绕过 LLM 直接写入，其余都经真实 HTTP。
import assert from 'node:assert/strict'
import { join } from 'node:path'
import { fetchIdempotent } from './fixtures/route-fetch.mjs'

async function chooseSpace(page, layout, name) {
  await layout.getByRole('combobox', { name: '记忆空间', exact: true }).click()
  await page.getByRole('option', { name, exact: false }).click()
}

async function saveDialog(dialog) {
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
}

export async function verifyMemoryLifecycle({ page, base, api, runtime, report, output }) {
  await page.goto(base + '/#/memory')
  const layout = page.locator('.page-memory [data-page-template="feature"]')
  await layout.waitFor()
  // 默认视图是演算（懒加载，画布由 MemoryEvolution 负责），时间线和整理记录在旁边。
  await layout.getByRole('tab', { name: '演算', exact: true }).waitFor()
  assert.equal(
    await layout.getByRole('tab', { name: '演算', exact: true }).getAttribute('aria-selected'),
    'true',
  )
  await layout.locator('[data-memory-detail]').waitFor()
  report.checks.push('Memory opens on the evolution tab with a detail region below it')

  const more = layout.getByRole('button', { name: '更多操作', exact: true })
  await more.click()
  await page.getByRole('menuitem', { name: '新建记忆空间', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: '新建记忆空间', exact: true })
  await dialog.getByLabel('空间名称', { exact: true }).fill('PI memory space')
  await saveDialog(dialog)
  const space = (await api('/api/memory')).spaces.find((s) => s.name === 'PI memory space')
  assert.ok(space?.id)
  await layout.getByRole('combobox', { name: '记忆空间', exact: true }).waitFor()
  assert.match(
    await layout.getByRole('combobox', { name: '记忆空间', exact: true }).innerText(),
    /PI memory space/,
  )

  await layout.getByRole('tab', { name: '时间线', exact: true }).click()
  await layout.getByRole('button', { name: '添加记忆', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '添加记忆', exact: true })
  await dialog.getByLabel('标题', { exact: true }).fill('PI memory searchable')
  await dialog
    .getByRole('textbox', { name: '内容', exact: true })
    .fill('Persistent acceptance fixture with nebula keyword.')
  await saveDialog(dialog)
  const row = layout.locator('[data-memory-node]').filter({ hasText: 'PI memory searchable' })
  await row.waitFor()
  // 今天写的记忆落在“今天”这一组。
  await layout.locator('[data-memory-group="today"]').waitFor()
  const toggle = row.locator('[data-feature-row-toggle]')
  assert.equal(await toggle.getAttribute('aria-expanded'), 'false')
  await toggle.click()
  assert.equal(await toggle.getAttribute('aria-expanded'), 'true')
  const detail = row.getByRole('region', { name: 'PI memory searchable' })
  await detail.getByRole('button', { name: '编辑', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '编辑记忆', exact: true })
  await dialog
    .getByRole('textbox', { name: '内容', exact: true })
    .fill('Edited nebula memory is durable.')
  await saveDialog(dialog)
  await detail.getByText('Edited nebula memory is durable.', { exact: true }).waitFor()
  report.checks.push('Timeline expands a memory into its detail and edits it through a dialog')

  await detail.getByRole('button', { name: '钉住', exact: true }).click()
  await row.getByText('已钉住', { exact: true }).first().waitFor()
  assert.equal(
    (await api('/api/memory?spaceId=' + encodeURIComponent(space.id))).nodes[0].pinned,
    true,
  )
  await detail.getByRole('button', { name: '取消钉住', exact: true }).click()
  await detail.getByRole('button', { name: '钉住', exact: true }).waitFor()
  report.checks.push('A memory can be pinned and unpinned from its detail')

  await page.reload()
  await layout.waitFor()
  await layout.getByRole('tab', { name: '时间线', exact: true }).waitFor()
  // 上次看的视图记住了。
  assert.equal(
    await layout.getByRole('tab', { name: '时间线', exact: true }).getAttribute('aria-selected'),
    'true',
  )
  await chooseSpace(page, layout, 'PI memory space')
  await row.waitFor()

  const search = page.getByPlaceholder('搜索记忆或文件', { exact: true })
  await Promise.all([
    page.waitForResponse(
      (r) =>
        new URL(r.url()).pathname === '/api/memory' &&
        new URL(r.url()).searchParams.get('query') === 'nebula',
    ),
    search.fill('nebula'),
  ])
  await row.waitFor()
  await Promise.all([
    page.waitForResponse(
      (r) =>
        new URL(r.url()).pathname === '/api/memory' &&
        new URL(r.url()).searchParams.get('query') === 'missing-unique-keyword',
    ),
    search.fill('missing-unique-keyword'),
  ])
  await row.waitFor({ state: 'hidden' })
  await search.fill('')
  await row.waitFor()
  // 晚到的旧搜索结果不能盖掉新的结果。
  const searchRelease = Promise.withResolvers()
  await page.route('**/api/memory?**', async (route) => {
    if (new URL(route.request().url()).searchParams.get('query') !== 'nebula')
      return route.continue()
    const response = await fetchIdempotent(route)
    await searchRelease.promise
    await route.fulfill({ response })
  })
  try {
    await Promise.all([
      page.waitForRequest((r) => new URL(r.url()).searchParams.get('query') === 'nebula'),
      search.fill('nebula'),
    ])
    await Promise.all([
      page.waitForResponse(
        (r) => new URL(r.url()).searchParams.get('query') === 'missing-race-keyword',
      ),
      search.fill('missing-race-keyword'),
    ])
    await row.waitFor({ state: 'hidden' })
    const staleFinished = page.waitForEvent('requestfinished', {
      predicate: (r) => new URL(r.url()).searchParams.get('query') === 'nebula',
    })
    searchRelease.resolve()
    await staleFinished
    await page.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
    )
    assert.equal(await row.count(), 0)
    assert.equal(await search.inputValue(), 'missing-race-keyword')
  } finally {
    searchRelease.resolve()
    await page.unroute('**/api/memory?**')
  }
  await search.fill('')
  await row.waitFor()
  report.checks.push('Late memory search responses cannot restore nodes from a superseded query')

  await more.click()
  await page.getByRole('menuitem', { name: '重命名记忆空间', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '重命名记忆空间', exact: true })
  await dialog.getByLabel('空间名称', { exact: true }).fill('PI renamed memory space')
  await saveDialog(dialog)
  assert.equal(
    (await api('/api/memory')).spaces.find((s) => s.id === space.id).name,
    'PI renamed memory space',
  )
  report.checks.push(
    'Memory UI creates a space and memory, edits and searches it, renames its space, and reloads durable data',
  )

  // 记忆不需要确认：抽取到的记忆直接出现，页面上没有确认或忽略的按钮。
  runtime.memory.remember({
    spaceId: space.id,
    title: 'PI extracted memory',
    content: 'PI extracted memory durable fixture',
    evidence: 'PI extracted memory',
    type: 'fact',
    sourceType: 'conversation',
    evidenceRole: 'user',
    topic: 'pi.extracted.memory',
  })
  await layout.getByRole('button', { name: '刷新', exact: true }).click()
  await layout.locator('[data-memory-node]').filter({ hasText: 'PI extracted memory' }).waitFor()
  assert.equal(await layout.getByRole('button', { name: '确认', exact: true }).count(), 0)
  assert.equal(await layout.getByRole('button', { name: '忽略', exact: true }).count(), 0)
  assert.deepEqual((await api('/api/memory')).candidates, [])
  await page.screenshot({ path: join(output, 'memory-lifecycle.png') })
  report.checks.push('An extracted memory appears directly with nothing to confirm or ignore')

  // 被替换的旧版本进“历史版本”，详情里能看到版本历史。
  runtime.memory.remember({
    spaceId: space.id,
    title: 'PI extracted memory',
    content: 'PI extracted memory changed',
    type: 'fact',
    sourceType: 'conversation',
    evidenceRole: 'user',
    topic: 'pi.extracted.memory',
  })
  await layout.getByRole('button', { name: '刷新', exact: true }).click()
  const extracted = layout
    .locator('[data-memory-node]')
    .filter({ hasText: 'PI extracted memory changed' })
  await extracted.waitFor()
  await extracted.locator('[data-feature-row-toggle]').click()
  await extracted.locator('[data-memory-history] li').nth(1).waitFor()
  assert.equal(await extracted.locator('[data-memory-history] li').count(), 2)
  await layout.getByRole('radio', { name: /历史版本/ }).click()
  await layout
    .locator('[data-memory-node]')
    .filter({ hasText: 'PI extracted memory durable fixture' })
    .waitFor()
  await layout.getByRole('radio', { name: /当前/ }).click()
  await row.waitFor()
  report.checks.push('A replaced fact moves to past versions and shows in the version history')

  // 时间轴是演算视图的主角：有记忆时一进来就要在首屏里，不能靠滚动才找得到。
  await layout.getByRole('tab', { name: '演算', exact: true }).click()
  const evolution = layout.locator('[data-memory-evolution]')
  await evolution.waitFor()
  const axis = evolution.getByRole('slider')
  await axis.waitFor()
  const axisBox = await axis.boundingBox()
  assert.ok(
    axisBox && axisBox.y + axisBox.height <= page.viewportSize().height,
    '时间轴被挤出了首屏',
  )
  // 键盘能拖动时间轴，舞台读数跟着变。
  await page.keyboard.press('Escape')
  await evolution.locator('[data-memory-evolution-now]').click()
  await axis.focus()
  const atBefore = await evolution.getAttribute('data-at')
  await page.keyboard.press('Home')
  await page.waitForFunction(
    (before) =>
      document.querySelector('[data-memory-evolution]')?.getAttribute('data-at') !== before,
    atBefore,
  )
  await evolution.locator('[data-memory-evolution-now]').click()
  report.checks.push(
    'Memory evolution keeps the time axis above the fold and scrubs from the keyboard',
  )

  // 触屏双指捏合：走真实的触摸手势管线，纵向张开最容易被浏览器当成滚动页面。
  const touch = await page.context().newCDPSession(page)
  await touch.send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 2 })
  const stage = await evolution.boundingBox()
  const pinchAt = (spread) => {
    const x = stage.x + stage.width / 2
    const y = stage.y + stage.height * 0.45
    return [
      { x, y: y - spread, id: 0 },
      { x, y: y + spread, id: 1 },
    ]
  }
  const scrollTopBefore = await page.evaluate(
    () => document.querySelector('[data-feature-scroller]')?.scrollTop ?? 0,
  )
  await touch.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: pinchAt(40) })
  for (const spread of [55, 70, 85]) {
    await touch.send('Input.dispatchTouchEvent', {
      type: 'touchMove',
      touchPoints: pinchAt(spread),
    })
  }
  await touch.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
  await page.waitForFunction(
    () =>
      Number(document.querySelector('[data-memory-evolution]')?.getAttribute('data-zoom')) > 1.5,
  )
  assert.equal(
    await page.evaluate(() => document.querySelector('[data-feature-scroller]')?.scrollTop ?? 0),
    scrollTopBefore,
    '捏合不应滚动页面',
  )
  assert.equal(await evolution.locator('[data-memory-evolution-card]').count(), 0)
  await touch.send('Emulation.setTouchEmulationEnabled', { enabled: false })
  await touch.detach()
  report.checks.push('Memory evolution zooms with a two-finger pinch without scrolling the page')

  await layout.getByRole('tab', { name: '整理记录', exact: true }).click()
  const activity = layout.locator('[data-memory-event]')
  await activity.first().waitFor()
  await layout.locator('[data-memory-event="supersede"]').first().waitFor()
  await layout.getByRole('tab', { name: '时间线', exact: true }).click()
  report.checks.push('The activity tab lists what changed')

  await row.waitFor()
  await row.locator('[data-feature-row-toggle]').click()
  await row
    .getByRole('region', { name: 'PI memory searchable' })
    .getByRole('button', { name: '删除', exact: true })
    .click()
  dialog = page.getByRole('dialog', { name: '删除记忆', exact: true })
  await dialog.getByText('历史版本').waitFor()
  await dialog.getByRole('button', { name: '删除', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  await row.waitFor({ state: 'hidden' })

  await more.click()
  await page.getByRole('menuitem', { name: '删除记忆空间', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '删除记忆空间', exact: true })
  await dialog.getByRole('button', { name: '删除', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  assert.ok(!(await api('/api/memory')).spaces.some((s) => s.id === space.id))
  await page.reload()
  await layout.getByRole('tab', { name: '时间线', exact: true }).waitFor()
  await layout.getByRole('combobox', { name: '记忆空间', exact: true }).click()
  assert.equal(await page.getByRole('option', { name: /PI renamed memory space/ }).count(), 0)
  await page.keyboard.press('Escape')
  report.checks.push(
    'Memory and space deletion use confirmation dialogs and stay deleted after reload',
  )
}
