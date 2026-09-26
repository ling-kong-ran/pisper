// Browser + real isolated Runtime memory lifecycle; candidate seeding bypasses the LLM only.
import assert from 'node:assert/strict'
import { join } from 'node:path'

export async function verifyMemoryLifecycle({ page, base, api, runtime, report, output }) {
  await page.goto(base + '/#/memory')
  const layout = page.locator('.memory-layout')
  await layout.waitFor()
  await layout.getByRole('button', { name: '新建星域', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: '新建星域', exact: true })
  await dialog.getByLabel('星域名称', { exact: true }).fill('PI memory space')
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  const space = (await api('/api/memory')).spaces.find((s) => s.name === 'PI memory space')
  assert.ok(space?.id)
  await page.getByRole('button', { name: '点亮星辰', exact: true }).first().click()
  dialog = page.getByRole('dialog', { name: '点亮星辰', exact: true })
  await dialog.getByLabel('星辰名称', { exact: true }).fill('PI memory searchable')
  await dialog
    .getByRole('textbox', { name: '星忆内容', exact: true })
    .fill('Persistent acceptance fixture with nebula keyword.')
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  const details = layout.locator('.detail-stack')
  await details.getByRole('heading', { name: 'PI memory searchable', exact: true }).waitFor()
  await details.getByRole('button', { name: '编辑', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '编辑星辰', exact: true })
  await dialog
    .getByRole('textbox', { name: '星忆内容', exact: true })
    .fill('Edited nebula memory is durable.')
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  await details.getByText('Edited nebula memory is durable.', { exact: true }).waitFor()
  await page.reload()
  await layout.getByRole('button', { name: /^PI memory space / }).click()
  await details.getByText('Edited nebula memory is durable.', { exact: true }).waitFor()
  const search = page.getByPlaceholder('搜索星辰或文件', { exact: true })
  await Promise.all([
    page.waitForResponse(
      (r) =>
        new URL(r.url()).pathname === '/api/memory' &&
        new URL(r.url()).searchParams.get('query') === 'nebula',
    ),
    search.fill('nebula'),
  ])
  await details.getByRole('heading', { name: 'PI memory searchable', exact: true }).waitFor()
  await Promise.all([
    page.waitForResponse(
      (r) =>
        new URL(r.url()).pathname === '/api/memory' &&
        new URL(r.url()).searchParams.get('query') === 'missing-unique-keyword',
    ),
    search.fill('missing-unique-keyword'),
  ])
  await details
    .getByRole('heading', { name: 'PI memory searchable', exact: true })
    .waitFor({ state: 'hidden' })
  await search.fill('')
  await details.getByRole('heading', { name: 'PI memory searchable', exact: true }).waitFor()
  // Hold an older successful search until the newer empty search has rendered.
  const searchRelease = Promise.withResolvers()
  await page.route('**/api/memory?**', async (route) => {
    if (new URL(route.request().url()).searchParams.get('query') !== 'nebula')
      return route.continue()
    const response = await route.fetch()
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
    await details
      .getByRole('heading', { name: 'PI memory searchable', exact: true })
      .waitFor({ state: 'hidden' })
    const staleFinished = page.waitForEvent('requestfinished', {
      predicate: (r) => new URL(r.url()).searchParams.get('query') === 'nebula',
    })
    searchRelease.resolve()
    await staleFinished
    await page.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
    )
    assert.equal(
      await details.getByRole('heading', { name: 'PI memory searchable', exact: true }).count(),
      0,
    )
    assert.equal(await search.inputValue(), 'missing-race-keyword')
  } finally {
    searchRelease.resolve()
    await page.unroute('**/api/memory?**')
  }
  await search.fill('')
  await details.getByRole('heading', { name: 'PI memory searchable', exact: true }).waitFor()
  report.checks.push('Late memory search responses cannot restore nodes from a superseded query')
  await layout.getByRole('button', { name: '重命名', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '重命名星域', exact: true })
  await dialog.getByLabel('星域名称', { exact: true }).fill('PI renamed memory space')
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  assert.equal(
    (await api('/api/memory')).spaces.find((s) => s.id === space.id).name,
    'PI renamed memory space',
  )
  report.checks.push(
    'Memory UI creates a space and memory, edits and searches it, renames its space, and reloads durable data',
  )

  await api('/api/settings/memory', 'PATCH', { autoApproveConfidence: 100 })
  for (const [title, action] of [
    ['PI candidate approve', '确认'],
    ['PI candidate reject', '忽略'],
  ]) {
    runtime.memory.propose({
      spaceId: space.id,
      title,
      content: title + ' durable fixture',
      evidence: title,
      type: 'fact',
      sourceType: 'conversation',
      confidence: 0.4,
      topic: title,
    })
    await layout.getByRole('button', { name: '刷新', exact: true }).click()
    const candidate = layout.locator('.memory-candidate').filter({ hasText: title })
    await candidate.waitFor()
    await candidate.getByRole('button', { name: action, exact: true }).click()
    await candidate.waitFor({ state: 'hidden' })
    const state = await api('/api/memory?spaceId=' + encodeURIComponent(space.id))
    assert.equal(
      state.nodes.some((n) => n.title === title),
      action === '确认',
    )
    assert.ok(!state.candidates.some((n) => n.title === title))
  }
  await page.screenshot({ path: join(output, 'memory-lifecycle.png') })
  report.checks.push(
    'Memory candidate acceptance persists a searchable node; rejection never enters recall; inbox actions run through real HTTP endpoints',
  )

  await details.getByRole('button', { name: '删除', exact: true }).click()
  dialog = page.getByRole('dialog', { name: '删除星辰', exact: true })
  await dialog.getByRole('button', { name: '删除', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  await layout
    .locator('.memory-space-actions')
    .getByRole('button', { name: '删除', exact: true })
    .click()
  dialog = page.getByRole('dialog', { name: '删除星域', exact: true })
  await dialog.getByRole('button', { name: '删除', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  assert.ok(!(await api('/api/memory')).spaces.some((s) => s.id === space.id))
  await page.reload()
  await layout.waitFor()
  assert.equal(await layout.getByRole('button', { name: /^PI renamed memory space / }).count(), 0)
  report.checks.push(
    'Memory and space deletion use confirmation dialogs and stay deleted after reload',
  )
}
