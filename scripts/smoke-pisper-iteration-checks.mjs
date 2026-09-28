// 新一轮界面行为验收；调用方必须提供隔离运行时和本地 CLI 导入 mock。
import assert from 'node:assert/strict'
import { join } from 'node:path'
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
  await page.goto(base + '/#/chat')
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

  await page.goto(base + '/#/config/interface')
  const fontGroup = page.getByRole('radiogroup', { name: '字体大小', exact: true })
  for (const [label, scale, size] of [
    ['小', 'small', 13.5],
    ['大', 'large', 14.5],
    ['标准', 'default', 14],
  ]) {
    await fontGroup.getByRole('radio', { name: label, exact: true }).click()
    await page.waitForFunction(
      (scale) => document.documentElement.dataset.fontScale === scale,
      scale,
    )
    const dimensions = await page
      .getByRole('button', { name: '设置', exact: true })
      .evaluate((el) => ({
        font: parseFloat(getComputedStyle(el.querySelector('span')).fontSize),
        height: el.getBoundingClientRect().height,
        icon: el.querySelector('svg').getBoundingClientRect().width,
        root: getComputedStyle(document.documentElement).fontSize,
      }))
    assert.deepEqual(dimensions, { font: size, height: 44, icon: 16, root: '16px' })
  }
  await page.reload()
  await fontGroup.waitFor()
  assert.equal(
    await fontGroup.getByRole('radio', { name: '标准', exact: true }).getAttribute('aria-checked'),
    'true',
  )
  report.checks.push(
    'Font scales are limited to 13.5/14/14.5px and persist; root spacing, 44px Settings row and 16px icon remain unchanged',
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
