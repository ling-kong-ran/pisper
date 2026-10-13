// Runs against the production app and real isolated Runtime. The caller supplies a controlled
// desktop bridge: these checks cover layout and dispatch, not native OS window operations.
import assert from 'node:assert/strict'

export async function verifyDesktopHeader(page, session) {
  const brand = page.getByTestId('workbench-brand-toggle')
  assert.equal(await brand.count(), 1, 'The sidebar logo must be an accessible toggle')
  const originalExpanded = await brand.getAttribute('aria-expanded')
  if (originalExpanded !== 'true') await brand.click()
  await page.waitForFunction(() => document.querySelector('#workbench-sidebar').clientWidth > 200)
  const header = await page.getByTestId('workbench-brand-header').boundingBox()
  const tabs = await page.locator('.dv-tabs-and-actions-container').first().boundingBox()
  assert.ok(Math.abs(header.y - tabs.y) <= 1 && Math.abs(header.height - tabs.height) <= 1)
  assert.ok((await brand.locator('svg').boundingBox()).height <= 22)
  await brand.click()
  await page.waitForFunction(() => document.querySelector('#workbench-sidebar').clientWidth < 100)
  assert.equal(await brand.getAttribute('aria-expanded'), 'false')
  await page.reload()
  await brand.waitFor()
  assert.equal(
    await brand.getAttribute('aria-expanded'),
    'false',
    'Collapsed state survives reload',
  )
  await brand.focus()
  await page.keyboard.press('Enter')
  await page.waitForFunction(() => document.querySelector('#workbench-sidebar').clientWidth > 200)
  assert.equal(await brand.getAttribute('aria-expanded'), 'true')
  assert.equal(await page.locator('#workbench-sidebar-collapse').count(), 0)
  assert.equal(await page.locator('#workbench-sidebar [data-testid="theme-toggle"]').count(), 1)

  const tab = () => page.getByRole('tab', { name: session.name, exact: true }).first()
  const dialog = page.getByRole('dialog', { name: '重命名会话', exact: true })
  const isActive = () =>
    tab().evaluate(
      (node) =>
        node.getAttribute('aria-selected') === 'true' && Boolean(node.closest('.dv-active-group')),
    )
  await page.getByRole('tab').filter({ hasNotText: session.name }).first().click()
  assert.equal(await isActive(), false)
  await tab().click()
  assert.equal(await isActive(), true)
  await page.getByRole('tabpanel', { name: session.name, exact: true }).waitFor()
  assert.equal(await dialog.count(), 0, 'Single click only activates the tab')
  await tab().dblclick()
  await dialog.waitFor()
  assert.equal(await dialog.getByRole('textbox').inputValue(), session.name)
  await dialog.getByRole('textbox').fill('')
  assert.equal(await dialog.getByRole('button', { name: '保存', exact: true }).isDisabled(), true)
  await page.keyboard.press('Escape')
  await dialog.waitFor({ state: 'detached' })
  await tab().dblclick()
  const renamed = session.name + ' renamed'
  await dialog.getByRole('textbox').fill(renamed)
  const saved = page.waitForResponse(
    (response) =>
      response.url().endsWith(`/api/sessions/${session.id}`) &&
      response.request().method() === 'PATCH',
  )
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  assert.ok((await saved).ok())
  await page.getByRole('tab', { name: renamed, exact: true }).first().waitFor()
  await page.locator(`button[title^="${renamed} ·"]`).waitFor()
  await page.reload()
  const renamedTab = page.getByRole('tab', { name: renamed, exact: true }).first()
  await renamedTab.waitFor()
  await renamedTab.click({ button: 'right' })
  await page.getByRole('menuitem', { name: '重命名会话', exact: true }).click()
  await dialog.getByRole('textbox').fill(session.name)
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  await tab().waitFor()

  await page.goto(new URL('/#/config/models', page.url()).href)
  await page.getByRole('heading', { name: '设置', exact: true }).waitFor()
  const controls = page.locator('.main-surface .desktop-window-controls')
  await controls.waitFor()
  assert.equal(await page.locator('.desktop-window-controls').count(), 1)
  const bounds = await controls.boundingBox()
  assert.equal(await page.locator('#workbench-sidebar [data-testid="theme-toggle"]').count(), 1)
  assert.equal(await page.getByRole('button', { name: '切换终端', exact: true }).count(), 0)
  assert.ok(bounds.x + bounds.width <= page.viewportSize().width)
  for (const button of await controls.getByRole('button').all()) {
    assert.equal(
      await button.evaluate((node) => {
        const rect = node.getBoundingClientRect()
        return node.contains(
          document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2),
        )
      }),
      true,
    )
  }
  await page.getByTestId('workbench-home').click()
  await page.locator('.dv-tabs-and-actions-container .desktop-window-controls').waitFor()
  if (originalExpanded !== 'true') await brand.click()
}
