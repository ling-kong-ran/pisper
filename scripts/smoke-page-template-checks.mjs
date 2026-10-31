import assert from 'node:assert/strict'
import { join } from 'node:path'

// A6 功能页模板（插件 / 技能 / MCP 起）：标题上方“‹ 功能”、h1、工具行、列表行；居中 760 栏、自己的滚动区不横向溢出。
// 桌面：顶栏 52 只是拖拽区，控件都在它下面。手机外壳（≤390）：h1 只给读屏、“‹ 功能”与所有触控目标 ≥44。
const FEATURE_TEMPLATE_ROUTES = new Set(['/plugins', '/skills', '/mcp'])

async function verifyFeatureTemplate(page, route, width) {
  const template = page.locator('.page-content > [data-page-template="feature"]')
  await template.waitFor()
  // 等数据读完：分区出现（技能页在空环境里可能没有任何行，只有空状态）。
  await template.locator('[data-feature-section]').first().waitFor({ timeout: 30000 })
  const facts = await template.evaluate((root) => {
    const scroller = root.querySelector('[data-feature-scroller]')
    const column = root.querySelector('[data-feature-column]')
    const heading = root.querySelector('h1')
    const back = root.querySelector('[data-back-to-more]')
    const drag = root.querySelector(':scope > [data-window-drag-region]')
    const top = root.getBoundingClientRect().top
    const style = getComputedStyle(heading)
    // 开关的可视轨道是 34×20，热区由 ::after 扩到 58×44（基础组件约定），按热区计。
    const targets = [
      ...root.querySelectorAll('button, input, a[href], [role="switch"], [role="tab"]'),
    ]
      .filter((node) => node.getClientRects().length > 0)
      .map((node) => {
        // 搜索框的触控目标是包住它的 label（整条 44 高都能点进输入框），不是只有一行字高的 input 本身。
        const box = (
          node.matches('input') && node.closest('label') ? node.closest('label') : node
        ).getBoundingClientRect()
        const after = node.matches('[role="switch"]')
          ? Number.parseFloat(getComputedStyle(node, '::after').height) || 0
          : 0
        return {
          label:
            node.getAttribute('aria-label') || node.textContent.trim().slice(0, 24) || node.tagName,
          height: Math.max(box.height, after),
          top: box.top - top,
        }
      })
    return {
      overflow: scroller.scrollWidth - scroller.clientWidth,
      column: column.getBoundingClientRect().width,
      columnPadding:
        Number.parseFloat(getComputedStyle(column).paddingLeft) +
        Number.parseFloat(getComputedStyle(column).paddingRight),
      heading: heading.textContent,
      srOnly: style.position === 'absolute' && heading.getBoundingClientRect().width <= 1,
      backHeight: back?.getBoundingClientRect().height ?? 0,
      backText: back?.textContent.trim(),
      backLabel: back?.getAttribute('aria-label'),
      drag: drag ? drag.getBoundingClientRect().height : 0,
      targets,
    }
  })
  assert.ok(
    facts.overflow <= 1,
    `${width}px ${route} feature scroller overflows: ${facts.overflow}`,
  )
  assert.ok(
    facts.column - facts.columnPadding <= 760.5,
    `${width}px ${route} content column ${facts.column - facts.columnPadding}px`,
  )
  assert.equal(facts.backText, '功能', `${route} back text`)
  assert.equal(facts.backLabel, '返回功能', `${route} back label`)
  const phone = await page.evaluate(
    () =>
      document.documentElement.hasAttribute('data-phone-shell') ||
      Boolean(document.querySelector('[data-phone-shell]')),
  )
  if (phone) {
    assert.equal(
      facts.srOnly,
      true,
      `${width}px ${route} h1 is only for screen readers on the phone`,
    )
    assert.equal(facts.drag, 0, `${route} phone has no drag bar`)
    assert.ok(facts.backHeight >= 44, `${width}px ${route} ‹ 功能 ${facts.backHeight}px`)
    const small = facts.targets.filter((target) => target.height < 44)
    assert.deepEqual(small, [], `${width}px ${route} touch targets under 44px`)
  } else {
    assert.equal(facts.srOnly, false, `${width}px ${route} h1 visible on desktop`)
    assert.equal(facts.drag, 52, `${width}px ${route} 52px drag-only top bar`)
    assert.ok(
      facts.targets.every((target) => target.top >= 52),
      `${width}px ${route} controls stay below the drag bar: ${JSON.stringify(facts.targets.filter((target) => target.top < 52))}`,
    )
  }
}

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
        if (FEATURE_TEMPLATE_ROUTES.has(route)) await verifyFeatureTemplate(page, route, width)
        if (route === '/schedules') {
          const list = page.locator('[data-slot="page-list"]')
          await list.getByRole('button', { name: /Template verification/ }).click()
          const detail = page.locator('[data-slot="page-detail"]')
          await detail.waitFor({ state: 'visible' })
          // 列表/详情按容器宽度（760px）而不是视口切换单栏；新外壳没有侧栏和活动栏，
          // 800px 视口下内容区已够宽，两栏并排。手机宽度必须是单栏。
          const stacked = await list.evaluate(
            (el) =>
              el.closest('[class*="@container/list-detail"]').getBoundingClientRect().width < 760,
          )
          if (width <= 390) assert.equal(stacked, true, `${width}px schedules stack list/detail`)
          if (stacked) {
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
    // 刷新时应用壳的启动查询与设置页各读一次通知配置，每次注入的失败都记为预期。
    await page.route('**/api/settings/notifications', (route) => {
      report.expectedFailedApi.push({ path: '/api/settings/notifications', status: 503 })
      return route.fulfill({
        status: 503,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Template fixture load failure' }),
      })
    })
    await page.reload()
    await page.getByRole('alert').filter({ hasText: 'Template fixture load failure' }).waitFor()
    await page.unroute('**/api/settings/notifications')
    await page.getByRole('button', { name: '重试', exact: true }).click()
    await page
      .getByRole('alert')
      .filter({ hasText: 'Template fixture load failure' })
      .waitFor({ state: 'hidden' })
    report.checks.push(
      'Four page templates plus the A6 feature-page template (plugins / skills / MCP: ‹ 功能 above the h1, ≤760 column, no horizontal scroll, 52 drag-only bar on desktop, sr-only h1 and ≥44px touch targets on the phone): actual Chinese/English routes at 1600/1280/800/390/320px, large font, no duplicate host scrolling, list/detail native back, embedded catalog, and settings error/retry',
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
