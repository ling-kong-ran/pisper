import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { writeFile, mkdtemp, mkdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright-core'
import { DEFAULT_BRANCH } from '../shared/app-update.mjs'
// 始终使用全新的临时后端，不连接已安装应用，也不读取真实密钥。
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = await mkdtemp(join(tmpdir(), 'pisper-zcode-ui-'))
const dataDir = join(output, 'agent')
const workspace = join(output, 'workspace')
await mkdir(workspace)
process.env.PI_SKIP_VERSION_CHECK = '1'
process.env.PI_TELEMETRY = '0'
process.env.PISPER_AGENT_DIR = dataDir
process.env.PISPER_WORKSPACE_DIR = workspace
const { createPisperRuntime } = await import('../runtime/app-runtime.mjs')
const runtime = await createPisperRuntime({
  root,
  frontendRoot: join(root, 'dist'),
  runtimeCwd: workspace,
  dataDir,
  production: true,
  port: 0,
  host: '127.0.0.1',
  remote: { enabled: false },
})
const base = runtime.url
const provider = 'pi-control-ui-smoke'
const alternate = provider + '-alt'
const model = 'pi-ui-fixture'
const report = {
  backend: base,
  fixture: 'loopback-only OpenAI-compatible SSE, no external model',
  status: 'running',
  mockedServices: ['provider discovery', 'GitHub update check'],
  checks: [],
  requests: 0,
  stopConnectionClosed: false,
  pageErrors: [],
  failedApi: [],
}
let page, browser, sessionId
const fixture = createServer(async (req, res) => {
  try {
    if (req.method === 'GET') {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(
        JSON.stringify({
          object: 'list',
          data: [{ id: model, object: 'model', owned_by: 'local-test' }],
        }),
      )
      return
    }
    let raw = ''
    for await (const chunk of req) raw += chunk
    const body = JSON.parse(raw)
    report.requests++
    const last = body.messages?.findLast((x) => x.role === 'user')?.content
    const slow = JSON.stringify(last).includes('stop-test')
    const content = slow ? '正在生成停止测试' : '流式回复：验收通过'
    if (!body.stream) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(
        JSON.stringify({
          id: 'local-test',
          object: 'chat.completion',
          model,
          choices: [{ index: 0, message: { role: 'assistant', content }, finish_reason: 'stop' }],
          usage: { prompt_tokens: 12, completion_tokens: 8, total_tokens: 20 },
        }),
      )
      return
    }
    res.writeHead(200, {
      'content-type': 'text/event-stream; charset=utf-8',
      'cache-control': 'no-cache',
    })
    const chunk = (delta, finish_reason = null, usage) =>
      res.write(
        `data: ${JSON.stringify({ id: 'local-test', object: 'chat.completion.chunk', created: Math.floor(Date.now() / 1000), model, choices: [{ index: 0, delta, finish_reason }], ...(usage ? { usage } : {}) })}\n\n`,
      )
    chunk({ role: 'assistant', content: slow ? content : '流式回复：' })
    if (slow) {
      const timer = setInterval(() => {
        if (!res.destroyed) chunk({ content: ' …' })
      }, 500)
      res.on('close', () => {
        clearInterval(timer)
        report.stopConnectionClosed = true
      })
    } else {
      setTimeout(() => {
        if (!res.destroyed) {
          chunk({ content: '验收通过' })
          chunk({}, 'stop', { prompt_tokens: 12, completion_tokens: 8, total_tokens: 20 })
          res.end('data: [DONE]\n\n')
        }
      }, 1200)
    }
  } catch (error) {
    if (!res.headersSent) res.writeHead(500)
    res.end(String(error))
  }
})
await new Promise((resolve) => fixture.listen(0, '127.0.0.1', resolve))
async function api(path, method = 'GET', data) {
  const r = await fetch(base + path, {
    method,
    headers: { Origin: base, 'Content-Type': 'application/json' },
    body: data ? JSON.stringify(data) : undefined,
  })
  const text = await r.text()
  assert.ok(r.ok, `${method} ${path}: ${r.status} ${text.slice(0, 500)}`)
  return text ? JSON.parse(text) : null
}
try {
  await api('/api/providers', 'POST', {
    id: provider,
    name: 'PI Local UI Test',
    providerType: 'chat',
    api: 'openai-completions',
    baseUrl: `http://127.0.0.1:${fixture.address().port}/v1`,
    apiKey: 'local-test-only-not-a-secret',
    model,
    modelKind: 'chat',
    enabled: true,
    reasoning: true,
    thinkingLevels: ['off', 'low', 'medium', 'high'],
  })
  await api('/api/providers', 'POST', {
    id: alternate,
    name: 'PI Alternate UI Test',
    providerType: 'chat',
    api: 'openai-completions',
    baseUrl: `http://127.0.0.1:${fixture.address().port}/v1`,
    apiKey: 'local-test-only-not-a-secret',
    model: model + '-alt',
    modelKind: 'chat',
    enabled: true,
    reasoning: true,
    thinkingLevels: ['off', 'low', 'medium', 'high'],
  })
  for (const [suffix, reasoning, levels] of [
    ['-plain', false, []],
    ['-fixed', true, ['off']],
  ]) {
    await api('/api/providers', 'POST', {
      id: provider + suffix,
      name: `PI ${suffix} UI Test`,
      providerType: 'chat',
      api: 'openai-completions',
      baseUrl: `http://127.0.0.1:${fixture.address().port}/v1`,
      apiKey: 'local-test-only-not-a-secret',
      model: model + suffix,
      modelKind: 'chat',
      enabled: true,
      reasoning,
      ...(reasoning ? { thinkingLevels: levels } : {}),
    })
  }
  const config = await api('/api/config', 'PUT', { provider, setAsDefault: true })
  assert.equal(config.provider, provider)
  report.checks.push('real provider configuration API with isolated loopback fixture')
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_UI_BROWSER_PATH
      ? { executablePath: process.env.PISPER_UI_BROWSER_PATH }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    locale: 'zh-CN',
  })
  page = await context.newPage()
  page.on('pageerror', (e) => report.pageErrors.push(e.message))
  page.on('response', (r) => {
    if (r.url().includes('/api/') && r.status() >= 400)
      report.failedApi.push({ path: new URL(r.url()).pathname, status: r.status() })
  })
  page.on('request', (r) => {
    if (r.method() === 'POST' && new URL(r.url()).pathname === '/api/chat')
      sessionId = r.postDataJSON().sessionId
  })
  // 网络发现与 GitHub 更新不是本地界面验收目标，固定结果避免依赖网络及未推送的 HEAD。
  await page.route('**/api/providers/discovery', (r) => r.fulfill({ json: { providers: [] } }))
  await page.route('**/api/app-update*', (r) =>
    r.fulfill({
      json: {
        state: 'current',
        currentVersion: '0.0.0',
        currentCommit: '0'.repeat(40),
        availableCommit: '0'.repeat(40),
        behindBy: 0,
        branch: DEFAULT_BRANCH,
        notes: '',
        releaseDate: null,
        releaseUrl: '',
        canDownload: false,
        checkedAt: Date.now(),
        message: 'Local UI fixture: GitHub update checks are verified separately.',
      },
    }),
  )
  await page.addInitScript(() => {
    try {
      localStorage.setItem('pisper-language', 'zh-CN')
      if (!localStorage.getItem('pisper-ui')) localStorage.setItem('pisper-theme', 'light')
      localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
    } catch {}
  })
  await page.goto(base + '/#/chat')
  await page.getByRole('textbox', { name: '任务描述' }).waitFor({ timeout: 60000 })
  await page.getByRole('button', { name: /^在 .* 中新建会话$/ }).click()
  await page.locator('[data-brand=PI]').waitFor()
  const prompt = page.getByRole('textbox', { name: '任务描述' })
  assert.deepEqual(
    await page
      .locator('.focus-composer-visible-tools [data-composer-tool-id]')
      .evaluateAll((nodes) => nodes.map((node) => node.dataset.composerToolId)),
    ['permission', 'run-mode', 'model'],
  )
  assert.equal(
    await page.getByRole('button', { name: '审批模式：完全访问', exact: true }).count(),
    0,
  )
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  await page.getByRole('combobox', { name: '当前会话模型' }).waitFor()
  assert.equal(await page.getByRole('combobox', { name: '当前会话模型' }).isEnabled(), true)
  await page.getByRole('combobox', { name: '当前会话模型' }).click()
  await page.getByRole('option', { name: /pi-ui-fixture-alt/ }).click()
  await page
    .getByRole('combobox', { name: '当前会话模型' })
    .filter({ hasText: 'pi-ui-fixture-alt' })
    .waitFor()
  await page.getByRole('combobox', { name: '当前思考等级' }).click()
  await page.getByRole('option', { name: '深度', exact: true }).click()
  await page.getByRole('combobox', { name: '当前思考等级' }).filter({ hasText: '深度' }).waitFor()
  await page.keyboard.press('Escape')
  await page.reload()
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  await page
    .getByRole('combobox', { name: '当前会话模型' })
    .filter({ hasText: 'pi-ui-fixture-alt' })
    .waitFor({ timeout: 30000 })
  await page.getByRole('combobox', { name: '当前思考等级' }).filter({ hasText: '深度' }).waitFor()
  await page.keyboard.press('Escape')
  report.checks.push(
    'one model/reasoning panel updates both real session settings and persists after reload',
  )
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  for (const suffix of ['-plain', '-fixed']) {
    await page.getByRole('combobox', { name: '当前会话模型' }).click()
    await page.getByRole('option', { name: new RegExp(`pi-ui-fixture${suffix}`) }).click()
    await page
      .getByRole('combobox', { name: '当前会话模型' })
      .filter({ hasText: `pi-ui-fixture${suffix}` })
      .waitFor()
    await page.waitForFunction(
      () => !document.querySelector('[aria-label="当前会话模型"]')?.disabled,
    )
    await page.waitForFunction(
      () => document.querySelector('[aria-label="当前思考等级"]')?.disabled,
    )
  }
  await page.getByRole('combobox', { name: '当前会话模型' }).click()
  await page.getByRole('option', { name: /pi-ui-fixture-alt/ }).click()
  await page.waitForFunction(() => !document.querySelector('[aria-label="当前思考等级"]')?.disabled)
  await page.getByRole('combobox', { name: '当前思考等级' }).click()
  await page.getByRole('option', { name: '深度', exact: true }).click()
  await page.keyboard.press('Escape')
  report.checks.push(
    'non-reasoning and fixed-effort providers disable reasoning; returning to a supported model restores editing',
  )
  await page.getByRole('button', { name: /^执行模式 · Plan 模式/ }).click()
  await page.getByRole('menuitemradio', { name: /Goal 模式/ }).click()
  await page.getByRole('button', { name: /^执行模式 · Goal 模式/ }).waitFor()
  await page.reload()
  await page.getByRole('button', { name: /^执行模式 · Goal 模式/ }).click()
  await page.getByRole('menuitemradio', { name: /Plan 模式/ }).click()
  await page.getByRole('button', { name: /^执行模式 · Plan 模式/ }).waitFor()
  report.checks.push(
    'visible Plan/Goal selector changes backend mode, persists on reload, restores Plan',
  )
  await page.getByRole('button', { name: /^审批模式：/ }).click()
  await page.getByRole('menuitemradio', { name: /^自动审批/ }).click()
  await page.getByRole('button', { name: '审批模式：自动审批', exact: true }).waitFor()
  await page.reload()
  await page.getByRole('button', { name: '审批模式：自动审批', exact: true }).waitFor()
  await page.getByRole('button', { name: '审批模式：自动审批', exact: true }).click()
  await page.getByRole('menuitemradio', { name: /^审批后写入/ }).click()
  await page.getByRole('button', { name: '审批模式：审批后写入', exact: true }).waitFor()
  await page.getByRole('button', { name: '审批模式：审批后写入', exact: true }).click()
  await page.getByRole('menuitemradio', { name: /^完全访问/ }).click()
  const accessConfirmation = page.getByRole('dialog', { name: '启用完全访问', exact: true })
  await accessConfirmation.getByRole('button', { name: '取消', exact: true }).click()
  await page.getByRole('button', { name: '审批模式：审批后写入', exact: true }).click()
  await page.getByRole('menuitemradio', { name: /^完全访问/ }).click()
  await accessConfirmation.getByRole('button', { name: '启用完全访问', exact: true }).click()
  await page.getByRole('button', { name: '审批模式：完全访问', exact: true }).waitFor()
  await page.reload()
  await page.getByRole('button', { name: '审批模式：完全访问', exact: true }).click()
  await page.getByRole('menuitemradio', { name: /^审批后写入/ }).click()
  await page.getByRole('button', { name: '审批模式：审批后写入', exact: true }).waitFor()
  report.checks.push(
    'approval/auto/full access are real persisted settings; no elevated default; restored approval-required',
  )
  await page.getByRole('button', { name: '打开会话操作菜单', exact: true }).click()
  assert.equal(await page.getByRole('menuitem', { name: /拆分到|关闭标签/ }).count(), 0)
  await page.keyboard.press('Escape')
  report.checks.push('no split-window or uncloseable pane entries in session menu')
  await page.getByRole('button', { name: '展开快捷操作', exact: true }).click()
  await page.getByRole('button', { name: '自定义快捷方式', exact: true }).click()
  await page.getByRole('button', { name: '模型与智力：移入收纳区', exact: true }).click()
  await page.getByRole('button', { name: '关闭对话框', exact: true }).click()
  await page.reload()
  await prompt.waitFor()
  assert.equal(
    await page.locator('.focus-composer-visible-tools [data-composer-tool-id="model"]').count(),
    0,
  )
  await page.getByRole('button', { name: '展开快捷操作', exact: true }).click()
  await page
    .getByRole('toolbar', { name: '快捷操作' })
    .getByRole('button', { name: /^模型与智力 ·/ })
    .click()
  await page.getByRole('combobox', { name: '当前思考等级' }).waitFor()
  await page.keyboard.press('Escape')
  await page.getByRole('button', { name: '自定义快捷方式', exact: true }).click()
  await page.getByRole('button', { name: '恢复默认', exact: true }).click()
  await page.getByRole('button', { name: '关闭对话框', exact: true }).click()
  await page.keyboard.press('Escape')
  await page.locator('.focus-composer-visible-tools [data-composer-tool-id="model"]').waitFor()
  report.checks.push(
    'toolbar customize/move to overflow persists; restore defaults returns all three inline controls',
  )
  await page.getByRole('button', { name: /^(展开|收起)快捷操作$/ }).waitFor()
  if ((await page.locator('.composer-tools-trigger').getAttribute('aria-expanded')) === 'true')
    await page.locator('.composer-tools-trigger').click()
  await page.getByRole('button', { name: '展开快捷操作', exact: true }).waitFor()
  const beforeIme = report.requests
  await prompt.fill('中文输入测试')
  await prompt.dispatchEvent('compositionstart')
  await prompt.dispatchEvent('keydown', {
    key: 'Enter',
    code: 'Enter',
    keyCode: 229,
    isComposing: true,
    bubbles: true,
  })
  assert.equal(report.requests, beforeIme)
  assert.equal(await prompt.inputValue(), '中文输入测试')
  await prompt.dispatchEvent('compositionend')
  await prompt.press('End')
  await prompt.press('Shift+Enter')
  assert.ok((await prompt.inputValue()).includes('\n'))
  assert.equal(report.requests, beforeIme)
  await prompt.fill('')
  report.checks.push('IME-composition Enter does not submit; Shift+Enter inserts a newline')

  await page.getByRole('button', { name: '展开快捷操作', exact: true }).click()
  const tools = await page
    .locator('[data-composer-tool-id]')
    .evaluateAll((nodes) => nodes.map((n) => n.dataset.composerToolId))
  assert.ok(
    tools.includes('attachment') &&
      tools.includes('resource') &&
      tools.includes('run-mode') &&
      tools.includes('commands') &&
      tools.includes('compact-context'),
  )
  report.checks.push('plus tray exposes attachments/resources/visual/run-mode/commands/compaction')
  console.log('TOOLS', tools)
  await page.keyboard.press('Escape')
  await page.getByRole('button', { name: '打开会话上下文', exact: true }).click()
  await page.getByRole('tab', { name: '文件改动', exact: true }).waitFor()
  for (const name of ['文件改动', '计划', '网页预览'])
    assert.equal(await page.getByRole('tab', { name, exact: true }).count(), 1)
  await page.getByRole('tab', { name: '文件改动', exact: true }).focus()
  await page.keyboard.press('End')
  assert.equal(
    await page.getByRole('tab', { name: '网页预览', exact: true }).getAttribute('aria-selected'),
    'true',
  )
  await page.keyboard.press('Home')
  await page.keyboard.press('ArrowRight')
  assert.equal(
    await page.getByRole('tab', { name: '计划', exact: true }).getAttribute('aria-selected'),
    'true',
  )
  await page.getByRole('button', { name: '关闭会话上下文', exact: true }).last().click()
  await page.getByRole('button', { name: '审批模式：审批后写入', exact: true }).click()
  console.log('PERMISSIONS', await page.getByRole('menu').ariaSnapshot())
  await page.keyboard.press('Escape')
  report.checks.push('right context files/plan/web-preview keyboard tabs and close/reopen')
  for (const width of [320, 390, 768, 1440]) {
    await page.setViewportSize({ width, height: 900 })
    await page.waitForFunction((expected) => window.innerWidth === expected, width)
    const inline = page.locator('.focus-composer-visible-tools')
    for (const id of ['permission', 'run-mode', 'model'])
      assert.ok(
        await inline.locator(`[data-composer-tool-id="${id}"]`).isVisible(),
        `${id} at ${width}`,
      )
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth),
      false,
      `document overflow at ${width}`,
    )
    const footer = await page
      .locator('.focus-composer-footer')
      .evaluate((node) => ({ width: node.clientWidth, scroll: node.scrollWidth }))
    assert.ok(
      footer.scroll <= footer.width + 1,
      `composer clips controls at ${width}: ${JSON.stringify(footer)}`,
    )
    await prompt.focus()
    await page.screenshot({ path: join(output, `light-${width}.png`), animations: 'disabled' })
  }
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light')
  await page.getByRole('button', { name: /^主题：.*点击切换主题$/ }).click()
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark')
  await page.reload()
  await prompt.waitFor()
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark')
  await page.getByRole('button', { name: /^主题：.*点击切换主题$/ }).waitFor()
  await prompt.focus()
  await page.screenshot({ path: join(output, 'dark-1440.png'), animations: 'disabled' })
  report.checks.push(
    '320/390/768/1440 px: all three primary controls stay visible with no horizontal clipping; light/dark screenshots',
  )
  await prompt.fill('ping [pi-ui-sse]')
  await page.getByRole('button', { name: '发送消息', exact: true }).click()
  await page.getByText('流式回复：', { exact: true }).waitFor({ timeout: 60000 })
  await page.getByRole('button', { name: '停止', exact: true }).waitFor()
  report.checks.push(
    'actual composer sends prompt through Pisper HTTP/SSE',
    'partial text arrives before final response',
  )
  await page.getByText('流式回复：验收通过', { exact: true }).waitFor({ timeout: 30000 })
  await page.getByRole('button', { name: '发送消息', exact: true }).waitFor()
  assert.ok(sessionId)
  const history = await api(`/api/sessions/${sessionId}/messages?limit=50`)
  assert.match(JSON.stringify(history), /流式回复：验收通过/)
  await page.screenshot({ path: join(output, 'conversation.png') })
  await page.reload()
  await page.getByText('流式回复：验收通过', { exact: true }).waitFor({ timeout: 30000 })
  report.checks.push(
    'completed assistant message persisted by release backend',
    'history survives browser reload',
  )
  await api(`/api/sessions/${sessionId}`, 'PATCH', { name: 'PI background-run QA' })
  await page.reload()
  await page.getByText('PI background-run QA', { exact: true }).first().waitFor()
  await prompt.fill('stop-test [pi-ui-sse]')
  await page.getByRole('button', { name: '发送消息', exact: true }).click()
  await page
    .getByText(/正在生成停止测试/)
    .first()
    .waitFor({ timeout: 30000 })
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  assert.equal(await page.getByRole('combobox', { name: '当前会话模型' }).isDisabled(), true)
  assert.equal(await page.getByRole('combobox', { name: '当前思考等级' }).isDisabled(), true)
  await page.keyboard.press('Escape')
  report.checks.push('both combined settings are disabled during an active stream')

  await page.getByRole('button', { name: /^在 .* 中新建会话$/ }).click()
  await page.locator('[data-brand=PI]').waitFor()
  const secondaryId = await page.evaluate(() => localStorage.getItem('pisper-active-session'))
  assert.notEqual(secondaryId, sessionId)
  await api(`/api/sessions/${secondaryId}`, 'PATCH', { name: 'PI draft side-session' })
  await prompt.fill('第二会话未发送草稿')
  assert.equal(report.stopConnectionClosed, false)
  await page.getByRole('button', { name: 'PI background-run QA', exact: true }).click()
  await page
    .getByText(/正在生成停止测试/)
    .first()
    .waitFor()
  assert.equal(report.stopConnectionClosed, false)
  report.checks.push(
    'switching active session unmounts the view without aborting the backend stream',
  )
  await prompt.fill('排队与撤回验收')
  await prompt.press('Enter')
  await page.getByRole('button', { name: '撤回消息', exact: true }).waitFor({ timeout: 15000 })
  await page.getByRole('button', { name: '撤回消息', exact: true }).click()
  await page.waitForFunction(
    () => document.querySelector('textarea[aria-label="任务描述"]')?.value === '排队与撤回验收',
  )
  await prompt.fill('')
  report.checks.push('Enter while streaming queues input; withdrawing restores the draft')
  await page.getByRole('button', { name: '停止', exact: true }).click()
  await page.getByRole('button', { name: '发送消息', exact: true }).waitFor({ timeout: 30000 })
  await new Promise((done, reject) => {
    const started = Date.now()
    const timer = setInterval(() => {
      if (report.stopConnectionClosed) {
        clearInterval(timer)
        done()
      } else if (Date.now() - started > 5000) {
        clearInterval(timer)
        reject(new Error('upstream stream was not closed'))
      }
    }, 25)
  })
  assert.equal(report.stopConnectionClosed, true)
  report.checks.push('Stop button aborts backend run and closes upstream stream')
  await page.getByRole('button', { name: 'PI draft side-session', exact: true }).click()
  await page.waitForFunction(
    () => document.querySelector('textarea[aria-label="任务描述"]')?.value === '第二会话未发送草稿',
  )
  report.checks.push('per-session unsent drafts survive session switching')
  // 使用真实历史页操作，避免从测试进程直接删除仍被活动视图读取的会话。
  await page.goto(base + '/#/chat/history')
  await page.getByRole('heading', { level: 1, name: '历史会话', exact: true }).waitFor()
  await page.getByRole('button', { name: 'PI background-run QA 的更多操作', exact: true }).click()
  await page.getByRole('menuitem', { name: '重命名会话', exact: true }).click()
  const renameDialog = page.getByRole('dialog', { name: '重命名会话', exact: true })
  await renameDialog.getByRole('textbox', { name: '会话标题', exact: true }).fill('PI UI CRUD 验收')
  await renameDialog.getByRole('button', { name: '保存', exact: true }).click()
  await renameDialog.waitFor({ state: 'hidden' })
  await page.getByRole('button', { name: 'PI UI CRUD 验收 的更多操作', exact: true }).waitFor()
  await page.reload()
  await page.getByRole('button', { name: 'PI UI CRUD 验收 的更多操作', exact: true }).waitFor()
  report.checks.push('session rename through history UI persists across reload')
  for (const [id, name] of [
    [secondaryId, 'PI draft side-session'],
    [sessionId, 'PI UI CRUD 验收'],
  ]) {
    const actions = page.getByRole('button', { name: `${name} 的更多操作`, exact: true })
    await actions.click()
    await page.getByRole('menuitem', { name: '删除会话', exact: true }).click()
    const confirmation = page.getByRole('dialog', { name: '删除会话', exact: true })
    const [deleted] = await Promise.all([
      page.waitForResponse(
        (r) =>
          new URL(r.url()).pathname === `/api/sessions/${id}` && r.request().method() === 'DELETE',
      ),
      confirmation.getByRole('button', { name: '删除', exact: true }).click(),
    ])
    assert.ok(deleted.ok())
    await confirmation.waitFor({ state: 'hidden' })
    await actions.waitFor({ state: 'hidden' })
  }
  const sessions = await api('/api/sessions')
  assert.ok(!JSON.stringify(sessions).includes(sessionId))
  assert.ok(!JSON.stringify(sessions).includes(secondaryId))
  report.checks.push('session deletion through confirmed history UI and release API')
  const routes = [
    ['/chat/history', 'chatHistory', '历史会话'],
    ['/assets', 'assets', '资产'],
    ['/workflows', 'workflows', '工作流'],
    ['/workflows/new', 'workflowCreate', '新建工作流'],
    ['/schedules', 'schedules', '定时任务'],
    ['/plugins', 'plugins', '插件'],
    ['/mcp', 'mcp', 'MCP'],
    ['/skills', 'skills', '技能'],
    ['/decisions', 'decisions', '决策模型'],
    ['/memory', 'memory', '星忆'],
    ['/channels', 'channels', '渠道'],
    ['/components', 'config', '设置', 'interface-chat-layout'],
    ...[
      ['models', 'models-connections'],
      ['notifications', 'notifications-browser'],
      ['interface', 'interface-appearance'],
      ['shortcuts', 'shortcuts-bindings'],
      ['desktop-pet', 'desktop-pet-settings'],
      ['updates', 'updates-main'],
      ['remote-access', 'remote-access-main'],
      ['about', 'about-project'],
    ].map(([section, anchor]) => ['/config/' + section, 'config', '设置', anchor]),
  ]
  report.routes = []
  for (const [route, pageId, heading, anchor] of routes) {
    await page.goto(base + '/#' + route)
    await page
      .getByRole('heading', { level: 1, name: heading, exact: true })
      .waitFor({ timeout: 45000 })
    // Hash 导航不会等待 React 换页；核对目标根与设置卡片，不能误将旧页面判为通过。
    const selector = `.page-content.page-${pageId}`
    await page.locator(selector).waitFor()
    if (anchor) await page.locator(`${selector} [data-config-card="${anchor}"]`).waitFor()
    await page.waitForFunction((selector) => {
      const content = document.querySelector(selector)
      return content && content.innerText.length > 35 && !/^加载中/.test(content.innerText)
    }, selector)
    const text = await page.locator('main').innerText()
    assert.ok(
      !/Unexpected Application Error|Application error|Cannot read properties|Minified React error/.test(
        text,
      ),
      route,
    )
    assert.equal(await page.locator('main h1').first().innerText(), heading)
    report.routes.push({ route, heading, resolvedHash: new URL(page.url()).hash })
  }
  report.checks.push('all 20 release feature/configuration routes render without a React error')
  await page.goto(base + '/#/chat')
  await prompt.waitFor()
  await prompt.fill('layout and sidebar draft')
  const sidebar = page.locator('[data-slot="sidebar"][data-state]')
  const previousState = await sidebar.getAttribute('data-state')
  await page.getByRole('button', { name: 'Toggle Sidebar', exact: true }).first().click()
  await page.waitForFunction(
    (previous) =>
      document.querySelector('[data-slot="sidebar"][data-state]')?.getAttribute('data-state') !==
      previous,
    previousState,
  )
  await page.getByRole('button', { name: 'Toggle Sidebar', exact: true }).first().click()
  await page.waitForFunction(
    (previous) =>
      document.querySelector('[data-slot="sidebar"][data-state]')?.getAttribute('data-state') ===
      previous,
    previousState,
  )
  assert.equal(await prompt.inputValue(), 'layout and sidebar draft')
  report.checks.push(
    'one header click collapses sidebar and another restores it without losing the draft',
  )
  await page.getByRole('button', { name: '会话布局', exact: true }).click()
  const layoutDialog = page.getByRole('dialog', { name: '会话布局', exact: true })
  await layoutDialog.getByRole('button', { name: '导出', exact: true }).click()
  const exportDialog = page.getByRole('dialog', { name: '导出布局模板', exact: true })
  const template = JSON.parse(await exportDialog.getByRole('textbox').inputValue())
  assert.equal(template.version, 2)
  await exportDialog.getByRole('button', { name: 'Close', exact: true }).click()
  await exportDialog.waitFor({ state: 'hidden' })
  await layoutDialog.waitFor()
  await layoutDialog.getByRole('button', { name: '导入', exact: true }).click()
  const importDialog = page.getByRole('dialog', { name: '导入会话布局', exact: true })
  await importDialog.getByRole('textbox').fill(JSON.stringify(template))
  await importDialog.getByRole('button', { name: '导入并切换', exact: true }).click()
  await importDialog.waitFor({ state: 'hidden' })
  await layoutDialog.waitFor({ state: 'hidden' })
  await prompt.waitFor()
  assert.equal(await prompt.inputValue(), 'layout and sidebar draft')
  report.checks.push(
    'existing layout JSON export/import still works and preserves the draft; no new frontend import mode',
  )

  assert.deepEqual(report.pageErrors, [])
  assert.deepEqual(report.failedApi, [])
  report.status = 'passed'
  console.log(JSON.stringify(report, null, 2))
} catch (error) {
  report.status = 'failed'
  report.failure = String(error)
  if (page && !page.isClosed()) {
    await page.screenshot({ path: join(output, 'failure.png') })
    console.log((await page.locator('body').ariaSnapshot()).slice(-8000))
  }
  throw error
} finally {
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
  await browser?.close()
  fixture.closeAllConnections()
  await new Promise((resolve) => fixture.close(resolve))
  await runtime.close()
  for (const directory of [dataDir, workspace]) {
    assert.equal(dirname(directory), output, 'Unsafe temporary cleanup path')
    await rm(directory, { recursive: true, force: true })
  }
  console.log(`UI smoke report and screenshots: ${output}`)
}
