import assert from 'node:assert/strict'
import { join } from 'node:path'

export async function verifyPageTemplates({ page, base, api, workspace, output, report }) {
  const task = await api('/api/schedules', 'POST', {
    name: 'Template verification — a long task title for narrow screens',
    prompt: 'UI fixture only; never execute.',
    enabled: false,
    frequency: 'daily',
    time: '09:00',
    timezone: 'UTC',
    cwd: workspace,
    executionMode: 'full-access',
    notifications: [],
    notifyOn: 'failure',
  })
  const scheduleId = task.task.id
  const original = page.viewportSize()
  try {
    for (const width of [1600, 1280, 800, 390, 320]) {
      await page.setViewportSize({ width, height: 844 })
      for (const route of [
        '/config/interface',
        '/config/models',
        '/schedules',
        '/mcp',
        '/skills',
        '/channels',
        '/memory',
        '/assets',
        '/plugins',
        '/workflows',
        '/workflows/new',
        '/config/interface?view=widgets',
      ]) {
        await page.goto(base + '/#' + route)
        const frame = page.locator('.page-content [data-page-template]').first()
        await frame.waitFor({ timeout: 30000 })
        await page.evaluate(() => {
          document.documentElement.dataset.fontScale = 'large'
        })
        assert.equal(
          await page
            .locator('.page-content > [data-page-template] [data-page-template="collection"]')
            .count(),
          0,
          'embedded catalog must not introduce a second page scroller',
        )
        const sizes = await page.locator('.page-content').evaluate((el) => ({
          width: el.clientWidth,
          scroll: el.scrollWidth,
          height: el.clientHeight,
          scrollHeight: el.scrollHeight,
        }))
        assert.ok(
          sizes.scroll <= sizes.width + 1,
          `${width}px ${route} overflows horizontally: ${JSON.stringify(sizes)}`,
        )
        assert.ok(
          sizes.scrollHeight <= sizes.height + 1,
          `${route} app host must not own a second page scroll`,
        )
        if (route === '/schedules') {
          const list = page.locator('[data-slot="page-list"]')
          await list.getByRole('button', { name: /Template verification/ }).click()
          const detail = page.locator('[data-slot="page-detail"]')
          await detail.waitFor({ state: 'visible' })
          if (width <= 800) {
            assert.equal(await list.isVisible(), false)
            assert.equal(await page.evaluate(() => window.__pisperRequestBack()), true)
            await list.waitFor({ state: 'visible' })
            assert.equal(await detail.isVisible(), false)
            await list.getByRole('button', { name: /Template verification/ }).click()
            await detail.waitFor({ state: 'visible' })
          }
        }
      }
      await page.screenshot({
        path: join(output, `templates-${width}.png`),
        animations: 'disabled',
      })
    }
    await page.goto(base + '/#/config/interface')
    await page.locator('[data-config-card="interface-language"]').getByRole('combobox').click()
    await page.getByRole('option', { name: 'English', exact: true }).click()
    for (const width of [1280, 390, 320]) {
      await page.setViewportSize({ width, height: 844 })
      for (const route of [
        '/config/models',
        '/schedules',
        '/mcp',
        '/skills',
        '/channels',
        '/memory',
        '/assets',
        '/plugins',
        '/workflows',
        '/workflows/new',
        '/config/interface',
      ]) {
        await page.goto(base + '/#' + route)
        await page.locator('.page-content [data-page-template]').first().waitFor()
        const bounds = await page.locator('.page-content').evaluate((el) => ({
          width: el.clientWidth,
          scroll: el.scrollWidth,
          height: el.clientHeight,
          scrollHeight: el.scrollHeight,
        }))
        assert.ok(
          bounds.scroll <= bounds.width + 1 && bounds.scrollHeight <= bounds.height + 1,
          `English ${width}px ${route} overflow`,
        )
      }
    }
    await page.locator('[data-config-card="interface-language"]').getByRole('combobox').click()
    await page.getByRole('option', { name: 'Simplified Chinese', exact: true }).click()
    await page.setViewportSize({ width: 1280, height: 900 })
    await page.goto(base + '/#/config/notifications')
    await page.locator('[data-page-template="settings"]').waitFor()
    // 配置读取失败必须保留可重试错误，不显示为成功或空配置。
    await page.route('**/api/settings/notifications', (route) =>
      route.fulfill({
        status: 503,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Template fixture load failure' }),
      }),
    )
    report.expectedFailedApi.push({ path: '/api/settings/notifications', status: 503 })
    await page.reload()
    await page.getByRole('alert').filter({ hasText: 'Template fixture load failure' }).waitFor()
    await page.unroute('**/api/settings/notifications')
    await page.getByRole('button', { name: '重试', exact: true }).click()
    await page
      .getByRole('alert')
      .filter({ hasText: 'Template fixture load failure' })
      .waitFor({ state: 'hidden' })
    report.checks.push(
      'Four page templates: actual Chinese/English routes at 1600/1280/800/390/320px, large font, no duplicate host scrolling, list/detail native back, embedded catalog, and settings error/retry',
    )
  } finally {
    await page.unroute('**/api/settings/notifications')
    await api('/api/schedules/' + encodeURIComponent(scheduleId), 'DELETE')
    await page.setViewportSize(original)
    await page.evaluate(() => {
      document.documentElement.dataset.fontScale = 'default'
    })
  }
}
