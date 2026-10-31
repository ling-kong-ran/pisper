// 新一轮界面行为验收；调用方必须提供隔离运行时和本地 CLI 导入 mock。
import assert from 'node:assert/strict'
import { join } from 'node:path'
import { boardFixture, installBoardFixture } from './fixtures/board-fixture.mjs'
export async function verifyPisperIteration({
  page,
  base,
  api,
  report,
  output,
  provider,
  alternate,
}) {
  await page.goto(base + '/#/config/models')
  const panel = page.locator('[data-model-provider-split-panel]')
  await panel.waitFor()
  const connections = panel.getByRole('navigation', { name: '连接', exact: true })
  await connections.getByRole('button', { name: 'PI Local UI Test', exact: true }).click()
  const form = panel.locator('[data-provider-connection-editor]')
  const name = form.getByLabel('显示名称', { exact: true })
  await name.fill('PI unsaved connection draft')
  await connections.getByRole('button', { name: 'PI Alternate UI Test', exact: true }).click()
  await connections.getByRole('button', { name: 'PI Local UI Test', exact: true }).click()
  assert.equal(await name.inputValue(), 'PI unsaved connection draft')
  await name.fill('PI Local UI Test')
  await page.getByText('本地配置自动接入 · 新增 0 项 · 待检查 0 项', { exact: true }).waitFor()
  report.checks.push(
    'Provider switch preserves an unsaved connection draft in memory; local import counts interpolate',
  )

  const modelId = 'pi-image-capability-ui'
  await panel.getByRole('button', { name: '添加模型', exact: true }).click()
  let editor = page.getByRole('dialog', { name: '添加模型', exact: true })
  await editor.getByLabel('模型 ID', { exact: true }).fill(modelId)
  await editor.getByRole('checkbox', { name: '图像生成', exact: true }).click()
  await editor.getByRole('checkbox', { name: '对话', exact: true }).click()
  const savePath = '/api/providers/' + provider + '/models'
  await page.route('**' + savePath, (r) =>
    r.fulfill({ status: 503, json: { error: 'PI model save failure fixture' } }),
  )
  report.expectedFailedApi.push({ path: savePath, status: 503 })
  await editor.getByRole('button', { name: '保存修改', exact: true }).click()
  await editor.getByRole('alert').filter({ hasText: 'PI model save failure fixture' }).waitFor()
  assert.equal(await editor.getByLabel('模型 ID', { exact: true }).inputValue(), modelId)
  await page.unroute('**' + savePath)
  await editor.getByRole('button', { name: '保存修改', exact: true }).click()
  await editor.waitFor({ state: 'hidden' })
  let config = await api('/api/config')
  let connection = config.providers.find((p) => p.id === provider)
  assert.equal(connection.configured, true)
  assert.deepEqual(connection.models.find((m) => m.id === modelId).capabilities, ['image'])
  assert.notEqual(connection.defaultModel, modelId)
  // 会话页（/board/new 建好后换成真实 id）里的真实模型选择器。
  await page.goto(base + '/#/board/new')
  await page.waitForFunction(() => /^#\/board\/(?!new$)[^/]+$/.test(location.hash))
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  await page.locator('.model-effort-model').getByRole('combobox', { name: '当前会话模型' }).click()
  await page.getByRole('listbox').waitFor()
  assert.equal(await page.getByRole('option').filter({ hasText: modelId }).count(), 0)
  await page.keyboard.press('Escape')
  await page.keyboard.press('Escape')
  await page.goto(base + '/#/config/models')
  await connections.getByRole('button', { name: 'PI Local UI Test', exact: true }).click()
  report.checks.push('Image-only models remain absent from the actual chat model picker')
  await panel
    .locator('[data-provider-model="' + modelId + '"]')
    .getByRole('button', { name: '模型能力：' + modelId, exact: true })
    .click()
  editor = page.getByRole('dialog', { name: '编辑模型', exact: true })
  assert.equal(await editor.getByLabel('模型 ID', { exact: true }).isDisabled(), true)
  await editor.getByRole('checkbox', { name: '对话', exact: true }).click()
  const reasoning = editor.getByRole('checkbox', { name: '深度思考', exact: true })
  if ((await reasoning.getAttribute('aria-checked')) !== 'true') await reasoning.click()
  await editor.getByRole('checkbox', { name: '图片理解', exact: true }).click()
  await editor.getByLabel('上下文窗口（Token）', { exact: true }).fill('32000')
  await editor.getByLabel('最大输出（Token）', { exact: true }).fill('4096')
  await page.screenshot({ path: join(output, 'model-capabilities.png') })
  await editor.getByRole('button', { name: '保存修改', exact: true }).click()
  await editor.waitFor({ state: 'hidden' })
  await page.reload()
  await panel.waitFor()
  config = await api('/api/config')
  connection = config.providers.find((p) => p.id === provider)
  const model = connection.models.find((m) => m.id === modelId)
  assert.deepEqual(model.capabilities, ['chat', 'image'])
  assert.equal(model.reasoning, true)
  assert.deepEqual(model.input, ['text', 'image'])
  assert.equal(model.contextWindow, 32000)
  assert.equal(model.maxTokens, 4096)
  assert.equal(config.providers.find((p) => p.id === alternate).configured, true)
  report.checks.push(
    'Model editor saves image-only capabilities on the existing connection, preserves failed drafts, and persists chat+image/reasoning/vision/token limits after reload',
  )

  // 界面字号是 11–16px 的数字输入（旧的小/标准/大三档已退役）；设置目录（A3 起在侧栏里）的当前行
  // 随界面字号缩放，行高、图标与根字号保持不变。旧侧栏 36px 行随外壳退役，分区导航行高为 32px。
  await page.goto(base + '/#/config/interface')
  await page.getByText('高级', { exact: true }).click()
  const fontInput = page.getByRole('spinbutton', { name: '界面字号', exact: true })
  const activeSection = page
    .getByRole('navigation', { name: '设置导航', exact: true })
    .locator('button[aria-current="page"]')
  for (const size of [12, 16, 14]) {
    await fontInput.fill(String(size))
    await fontInput.press('Enter')
    await page.waitForFunction((size) => {
      const label = document.querySelector(
        'nav[aria-label="设置导航"] button[aria-current="page"] span',
      )
      return label && Math.abs(parseFloat(getComputedStyle(label).fontSize) - size) < 0.001
    }, size)
    const dimensions = await activeSection.evaluate((el) => ({
      font: parseFloat(getComputedStyle(el.querySelector('span')).fontSize),
      height: el.getBoundingClientRect().height,
      icon: el.querySelector('svg').getBoundingClientRect().width,
      root: getComputedStyle(document.documentElement).fontSize,
    }))
    assert.deepEqual(dimensions, { font: size, height: 32, icon: 16, root: '16px' })
  }
  await page.reload()
  await page.getByText('高级', { exact: true }).click()
  await fontInput.waitFor()
  assert.equal(await fontInput.inputValue(), '14')
  report.checks.push(
    'The numeric UI font size scales the active settings-section row and persists; root spacing, the 32px section row and 16px icon remain unchanged',
  )

  // 旧侧栏 36px 会话行随外壳退役；界面字号改为钉住全部代理的行（标题 base、状态文字 xs；A3 起行取代卡片，
  // 卡片的元信息行随泳道退役，等价的 xs 文字是标题后的状态 data-row-status）与会话页
  // 泳道条芯片（xs）的缩放，泳道条保持 52px、页面不横向滚动。看板与泳道条的数据由看板固定数据在
  // 浏览器边界提供（独立的浏览器上下文，不影响上面的页面），会话页用本次隔离 Runtime 里的真实会话。
  {
    // 前面的步骤可能已经删掉了带消息的会话；泳道条只需要一条 Runtime 认识的会话来打开会话页。
    const listed = (await api('/api/sessions')).sessions
    const real =
      listed.find((session) => session.messageCount > 0) ??
      listed[0] ??
      (await api('/api/sessions', 'POST', { name: 'PI font scale fixture' }))
    assert.ok(real?.id, 'the isolated runtime has a session for the session page')
    const context = await page
      .context()
      .browser()
      .newContext({
        viewport: { width: 1440, height: 900 },
        locale: 'zh-CN',
      })
    try {
      await context.addInitScript(() => {
        try {
          localStorage.setItem('pisper-language', 'zh-CN')
          localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
        } catch {}
      })
      await installBoardFixture(context, {
        sessions: [
          ...boardFixture().sessions,
          { ...real, pinned: false, unread: false, needsAttention: false, streaming: true },
        ],
      })
      const scaled = await context.newPage()
      const card = scaled.locator('[data-board-card="fx-standard"]')
      // 规格 v2：会话页的泳道条换成应用侧栏，侧栏行与界面同为 14（随界面字号缩放）。
      const row = scaled.locator('[data-shell-sidebar] [data-sidebar-row="fx-standard"]')
      // 字号断言取文本所在元素（没有子元素、文字等于标题）的计算字号。
      const titleFont = (scope, title) =>
        scope.evaluate((root, text) => {
          const node = [root, ...root.querySelectorAll('*')].find(
            (element) => !element.children.length && element.textContent.trim() === text,
          )
          return node ? parseFloat(getComputedStyle(node).fontSize) : null
        }, title)
      for (const size of [12, 16, 14]) {
        await scaled.goto(base + '/#/config/interface')
        await scaled.getByText('高级', { exact: true }).click()
        const input = scaled.getByRole('spinbutton', { name: '界面字号', exact: true })
        await input.fill(String(size))
        await input.press('Enter')
        await scaled.waitForFunction(
          (expected) =>
            getComputedStyle(document.documentElement).getPropertyValue('--ui-font-size').trim() ===
            `${expected}px`,
          size,
        )
        const xs = Math.round((12 * size) / 14)
        await scaled.goto(base + '/#/board')
        await card.waitFor({ timeout: 30000 })
        assert.equal(await titleFont(card, '补充登录模块的单元测试'), size, `card title at ${size}`)
        assert.equal(
          await card
            .locator('[data-row-status]')
            .evaluate((element) => parseFloat(getComputedStyle(element).fontSize)),
          xs,
          `row status text at ${size}`,
        )
        await scaled.goto(base + `/#/board/${real.id}`)
        await row.waitFor({ timeout: 30000 })
        assert.equal(await titleFont(row, '补充登录模块的单元测试'), size, `sidebar row at ${size}`)
        const layout = await scaled.evaluate(() => ({
          sidebar: Math.round(
            document.querySelector('[data-shell-sidebar]').getBoundingClientRect().width,
          ),
          horizontalScroll: document.documentElement.scrollWidth > innerWidth,
          root: getComputedStyle(document.documentElement).fontSize,
        }))
        assert.deepEqual(layout, { sidebar: 268, horizontalScroll: false, root: '16px' })
      }
      await scaled.screenshot({ path: join(output, 'sidebar-font-scale.png') })
    } finally {
      await context.close()
    }
  }
  report.checks.push(
    'The numeric UI font size scales all-agents rows (title base, status xs) and sidebar session rows (base) at 12/16/14 px while the sidebar stays 268px, the root stays 16px and nothing scrolls horizontally',
  )

  const openWorkflowSettings = async () => {
    await page.getByRole('button', { name: '工作流设置', exact: true }).click()
    await page.getByRole('dialog', { name: '工作流设置', exact: true }).waitFor()
  }
  const closeWorkflowSettings = async () => {
    await page.keyboard.press('Escape')
    await page.getByRole('dialog', { name: '工作流设置', exact: true }).waitFor({ state: 'hidden' })
  }
  await page.goto(base + '/#/workflows/new')
  await openWorkflowSettings()
  const workflowName = page.getByLabel('名称', { exact: true })
  await workflowName.fill('PI workflow save return fixture')
  await closeWorkflowSettings()
  await page.route('**/api/workflows', (r) =>
    r.request().method() === 'POST'
      ? r.fulfill({ status: 503, json: { error: 'PI workflow save failure fixture' } })
      : r.continue(),
  )
  report.expectedFailedApi.push({ path: '/api/workflows', status: 503 })
  await page.getByRole('button', { name: '保存草稿', exact: true }).click()
  await page.getByText('PI workflow save failure fixture', { exact: true }).first().waitFor()
  assert.ok(page.url().endsWith('/#/workflows/new'))
  await openWorkflowSettings()
  assert.equal(await workflowName.inputValue(), 'PI workflow save return fixture')
  await closeWorkflowSettings()
  await page.unroute('**/api/workflows')
  await page.getByRole('button', { name: '保存草稿', exact: true }).click()
  await page.waitForURL((url) => /^#\/workflows\/(?!new$)[^/]+$/.test(url.hash))
  const saved = (await api('/api/workflows')).workflows.find(
    (w) => w.name === 'PI workflow save return fixture',
  )
  assert.ok(saved?.id)
  assert.ok(page.url().endsWith('/#/workflows/' + saved.id))
  await page.goto(base + '/#/workflows')
  await page.getByText('PI workflow save return fixture', { exact: true }).first().waitFor()
  report.checks.push(
    'New workflow explicit save retains its canvas only after success; failed save preserves draft; saved workflow remains in the list',
  )

  await page.goto(base + '/#/workflows/new')
  await openWorkflowSettings()
  await workflowName.fill('PI quiet save fixture')
  await closeWorkflowSettings()
  await page.route('**/api/workflows/*/run', async (r) => {
    report.expectedFailedApi.push({ path: new URL(r.request().url()).pathname, status: 503 })
    await r.fulfill({ status: 503, json: { error: 'PI workflow run failure fixture' } })
  })
  await page.getByRole('button', { name: '试运行', exact: true }).click()
  await page
    .getByRole('dialog')
    .getByRole('textbox', { name: /^本次任务/ })
    .fill('工作流失败恢复验收')
  await page.getByRole('dialog').getByRole('button', { name: '运行', exact: true }).click()
  await page.getByText('PI workflow run failure fixture', { exact: true }).first().waitFor()
  await page.getByRole('dialog').waitFor({ state: 'hidden' })
  assert.match(page.url(), /#\/workflows\/[^/]+$/)
  assert.ok(!page.url().endsWith('/new'))
  await openWorkflowSettings()
  assert.equal(await workflowName.inputValue(), 'PI quiet save fixture')
  await closeWorkflowSettings()
  await page.unroute('**/api/workflows/*/run')
  report.checks.push(
    'Running a new workflow saves quietly to its editor URL, never jumps to the list, and preserves the saved workflow if execution fails',
  )
  const runnable = (await api('/api/workflows')).workflows.find(
    (w) => w.name === 'PI quiet save fixture',
  )
  assert.ok(runnable?.id)
  const configured = (await api('/api/config')).providers.find((p) => p.id === provider)
  await api('/api/workflows/' + runnable.id, 'PATCH', {
    ...runnable,
    model: { provider, model: configured.defaultModel },
    nodes: runnable.nodes.map((node) =>
      node.kind === 'prompt'
        ? { ...node, prompt: 'Reply with a short workflow acceptance message. Do not use tools.' }
        : node,
    ),
  })
  await page.reload()
  const runResponse = page.waitForResponse(
    (r) =>
      new URL(r.url()).pathname === '/api/workflows/' + runnable.id + '/run' &&
      r.request().method() === 'POST',
  )
  await page.getByRole('button', { name: '试运行', exact: true }).click()
  await page
    .getByRole('dialog')
    .getByRole('textbox', { name: /^本次任务/ })
    .fill('工作流运行验收')
  await page.getByRole('dialog').getByRole('button', { name: '运行', exact: true }).click()
  const started = await (await runResponse).json()
  assert.equal(started.started, true)
  // Poll the API in Node and retain the exact terminal snapshot for assertions.
  // A browser waitForFunction predicate returning a Promise can end before its
  // eventual boolean is true; it must not be used as asynchronous API polling.
  const deadline = Date.now() + 30000
  const terminalStatuses = ['completed', 'failed', 'cancelled', 'interrupted', 'waiting_approval']
  let finished
  do {
    finished = await api('/api/workflow-runs/' + started.run.id)
    if (terminalStatuses.includes(finished.status)) break
    await new Promise((resolve) => setTimeout(resolve, 150))
  } while (Date.now() < deadline)
  assert.equal(finished.status, 'completed', finished.error)
  assert.equal(finished.nodes.find((node) => node.kind === 'prompt').status, 'completed')
  assert.match(JSON.stringify(finished), /验收通过/)
  const openLatestWorkflowRun = async () => {
    const inspectorButton = page.getByRole('button', { name: '节点属性', exact: true })
    if (await inspectorButton.isVisible()) await inspectorButton.click()
    await page.locator('summary').filter({ hasText: '最近运行' }).click()
    await page.locator('.activity-row.completed').waitFor()
  }
  await openLatestWorkflowRun()
  await page.reload()
  await openLatestWorkflowRun()
  assert.ok(page.url().endsWith('/workflows/' + runnable.id))
  report.checks.push(
    'Workflow UI runs a saved prompt through the real runtime and loopback model; completed output and editor location survive reload',
  )
}
