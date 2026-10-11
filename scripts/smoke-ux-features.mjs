// 验证关键表单的创建、保存和错误恢复；接口采用 Rust 当前 JSON 契约的隔离 fixture。
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { createServer } from 'node:http'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const mocks = {
  '@/app/i18n/use-i18n': `
    const t = (key, values) => key + (values?.name ? ' '+values.name : '')
    export function useI18n() { return {t, language:'en-US'} }
    export const translateText = t`,
  '@/lib/http/api': `export async function apiJson(url, options) {
    const response = await fetch(url, options)
    const data = await response.json()
    if (!response.ok) throw new Error(data.error)
    return data
  }`,
}
const bundle = await build({
  entryPoints: [join(root, 'scripts/fixtures/ux-features.jsx')],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'browser',
  jsx: 'automatic',
  alias: { '@': join(root, 'src'), '@shared': join(root, 'shared') },
  define: { 'process.env.NODE_ENV': '"production"' },
  plugins: [
    {
      name: 'fixture-boundaries',
      setup(builder) {
        builder.onResolve({ filter: /.*/ }, (args) =>
          mocks[args.path] ? { path: args.path, namespace: 'mock' } : null,
        )
        builder.onLoad({ filter: /.*/, namespace: 'mock' }, (args) => ({
          contents: mocks[args.path],
          loader: 'js',
        }))
      },
    },
  ],
})
const notifications = Object.fromEntries(
  ['browser', 'feishu', 'weixin', 'qq', 'telegram'].map((key) => [key, { enabled: false }]),
)
const workflow = {
  id: 'published-workflow',
  name: 'Fixture workflow',
  description: '',
  revision: 1,
  inputs: [
    {
      id: 'uuid-1',
      name: 'environment',
      label: 'Environment',
      type: 'string',
      required: true,
      defaultValue: '',
    },
    {
      id: 'uuid-2',
      name: 'limit',
      label: 'Limit',
      type: 'number',
      required: true,
      defaultValue: 3,
    },
    {
      id: 'uuid-3',
      name: 'notify',
      label: 'Notify',
      type: 'boolean',
      required: true,
      defaultValue: false,
    },
  ],
}
const schedules = {
  tasks: [],
  runs: [],
  notificationTargets: notifications,
  workflows: [workflow],
  defaultCwd: '/fixture',
}
const channels = {
  providers: [],
  connections: { feishu: null, weixin: null, qq: null, telegram: null },
  scopes: [],
  models: [],
}
const skills = {
  skills: [
    {
      id: 'fixture-skill',
      name: 'Fixture skill',
      description: 'Fixture description',
      enabled: true,
      modelInvocationEnabled: false,
      removable: false,
    },
  ],
  packages: [],
  counts: { enabled: 1, modelInvocable: 0 },
  locations: { global: '/fixture/skills', project: '/fixture/project/.pisper/skills' },
}
const evidence = { creates: [], saves: [], credentials: 0, skillWrites: 0 }
const failures = { skillRead: true, skillWrite: true, skillReload: true, credentials: true }
const errors = []
let createHold
let releaseCreate
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, 'http://fixture')
    if (!url.pathname.startsWith('/api/')) {
      response.setHeader(
        'Content-Type',
        url.pathname === '/bundle.js' ? 'text/javascript' : 'text/html',
      )
      response.end(
        url.pathname === '/bundle.js'
          ? bundle.outputFiles[0].text
          : '<!doctype html><html><body><div id="root"></div><script type="module" src="/bundle.js"></script></body></html>',
      )
      return
    }
    response.setHeader('Content-Type', 'application/json')
    const send = (value, status = 200) => {
      response.statusCode = status
      response.end(JSON.stringify(value))
    }
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : {}
    if (url.pathname === '/api/schedules' && request.method === 'GET') return send(schedules)
    if (url.pathname.startsWith('/api/schedules') && ['POST', 'PATCH'].includes(request.method)) {
      if (body.targetType === 'workflow') {
        // Rust native_workflow::inputs::validate 只接受输入 name，不能混入 UI 的 UUID id。
        assert.deepEqual(Object.keys(body.workflowInputs).sort(), [
          'environment',
          'limit',
          'notify',
        ])
        assert.equal(typeof body.workflowInputs.environment, 'string')
        assert.ok(body.workflowInputs.environment.trim())
        assert.ok(Number.isFinite(Number(body.workflowInputs.limit)))
        assert.equal(typeof body.workflowInputs.notify, 'boolean')
      }
      let task
      if (request.method === 'POST') {
        evidence.creates.push(body)
        if (createHold) {
          createHold.started()
          await createHold.released
        }
        task = { ...body, id: `task-${schedules.tasks.length + 1}`, lastStatus: 'idle' }
        schedules.tasks.push(task)
      } else {
        evidence.saves.push(body)
        task = { ...schedules.tasks[0], ...body }
        schedules.tasks[0] = task
      }
      return send({ task, state: schedules })
    }
    if (url.pathname === '/api/channels' && request.method === 'GET') return send(channels)
    if (url.pathname === '/api/channels/telegram/onboarding' && request.method === 'POST') {
      if (!Object.keys(body).length)
        return send(
          { mode: 'manual', platform: 'telegram', fields: ['token'], required: ['token'] },
          201,
        )
      evidence.credentials++
      assert.equal(body.token, 'synthetic-fixture-token')
      if (failures.credentials) return send({ error: 'Rejected fixture credentials' }, 400)
      channels.connections.telegram = {
        enabled: true,
        status: 'connected',
        defaultCwd: '/fixture',
        replyModel: null,
        executionMode: 'full-access',
        runMode: 'plan',
        accessMode: 'all',
        ownerConfigured: false,
      }
      // service::complete_onboarding 返回领域目录，模型列表仅由 GET /api/channels 追加。
      const { models: _models, ...state } = channels
      return send({ ...state, templates: [] }, 201)
    }
    if (url.pathname === '/api/skills' && request.method === 'GET') {
      return failures.skillRead ? send({ error: 'Fixture skills unavailable' }, 500) : send(skills)
    }
    if (url.pathname === '/api/skills/fixture-skill' && request.method === 'PATCH') {
      evidence.skillWrites++
      if (failures.skillWrite) return send({ error: 'Fixture skill update failed' }, 409)
      Object.assign(skills.skills[0], body)
      return send(skills.skills[0])
    }
    if (url.pathname === '/api/skills/reload' && request.method === 'POST') {
      return failures.skillReload
        ? send({ error: 'Fixture skill reload failed' }, 500)
        : send(skills)
    }
    errors.push(`Unexpected API: ${request.method} ${url.pathname}`)
    send({ error: 'Unexpected fixture API' }, 404)
  } catch (error) {
    errors.push(error.message)
    response.statusCode = 500
    response.end(JSON.stringify({ error: error.message }))
  }
})
let browser
try {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const executablePath = process.env.PISPER_SMOKE_BROWSER || chromium.executablePath()
  assert.ok(existsSync(executablePath), 'Install Chromium or set PISPER_SMOKE_BROWSER.')
  browser = await chromium.launch({ headless: true, executablePath })
  const page = await browser.newPage({ viewport: { width: 1000, height: 1000 }, hasTouch: true })
  page.setDefaultTimeout(7000)
  page.on('pageerror', (error) => errors.push(error.message))
  const base = `http://127.0.0.1:${server.address().port}`
  const button = (name) => page.getByRole('button', { name, exact: true })
  await page.goto(base + '/?page=schedules')
  await page.getByLabel('schedules:schedulesPage.taskName', { exact: true }).fill('Fixture task')
  await page.getByLabel('schedules:schedulesPage.executionTarget').selectOption('workflow')
  assert.equal(await button('schedules:schedulesPage.createTask').isEnabled(), false)
  assert.equal(await page.getByLabel('Limit', { exact: false }).inputValue(), '3')
  assert.equal(await page.getByLabel('Notify', { exact: false }).isChecked(), false)
  await page.getByLabel('Environment', { exact: false }).fill('production')
  assert.equal(await button('schedules:schedulesPage.createTask').isEnabled(), true)
  await button('schedules:schedulesPage.createTask').click()
  await button('schedules:schedulesPage.saveTask').waitFor()
  assert.deepEqual(evidence.creates[0].workflowInputs, {
    environment: 'production',
    limit: 3,
    notify: false,
  })
  await page.getByLabel('schedules:schedulesPage.time', { exact: true }).fill('11:30')
  await page.getByRole('switch', { name: 'Fixture task' }).click()
  await page.getByLabel('Environment', { exact: false }).fill('staging')
  await page.getByLabel('Limit', { exact: false }).fill('8')
  await page.getByLabel('Notify', { exact: false }).check()
  await button('schedules:schedulesPage.saveTask').click()
  await page.waitForFunction(() =>
    window.evidence.notices.includes('schedules:schedulesPage.scheduledTaskSaved'),
  )
  assert.equal(evidence.saves[0].time, '11:30')
  assert.equal(evidence.saves[0].enabled, false)
  assert.deepEqual(evidence.saves[0].workflowInputs, {
    environment: 'staging',
    limit: '8',
    notify: true,
  })
  await page.reload()
  await button('schedules:schedulesPage.saveTask').waitFor()
  assert.equal(await page.getByLabel('Environment', { exact: false }).inputValue(), 'staging')
  assert.equal(
    await page.getByLabel('schedules:schedulesPage.time', { exact: true }).inputValue(),
    '11:30',
  )

  // 壳层新建动作使用真实 Dialog，键盘不能落到遮罩后的导航。
  const primary = button('Fixture primary action')
  await primary.click()
  const dialog = page.getByRole('dialog', { name: 'schedules:schedulesPage.newScheduledTask' })
  await dialog.waitFor()
  await page.waitForFunction(() =>
    document.querySelector('[role="dialog"]')?.contains(document.activeElement),
  )
  assert.equal(
    await dialog
      .getByLabel('schedules:schedulesPage.taskName', { exact: true })
      .evaluate((node) => node === document.activeElement),
    true,
  )
  for (const key of ['Tab', 'Shift+Tab']) {
    for (let index = 0; index < 12; index++) {
      await page.keyboard.press(key)
      assert.equal(await dialog.evaluate((node) => node.contains(document.activeElement)), true)
    }
  }
  await page.keyboard.press('Escape')
  await dialog.waitFor({ state: 'hidden' })
  await page.waitForFunction(
    (node) => node === document.activeElement,
    await primary.elementHandle(),
  )
  assert.equal(await primary.evaluate((node) => node === document.activeElement), true)
  await primary.click()
  await dialog.getByLabel('schedules:schedulesPage.taskName', { exact: true }).fill('Dialog task')
  await dialog.getByLabel('Prompt', { exact: true }).fill('Synthetic task')
  let started
  const createStarted = new Promise((resolve) => {
    started = resolve
  })
  createHold = {
    started,
    released: new Promise((resolve) => {
      releaseCreate = resolve
    }),
  }
  await dialog
    .getByRole('button', { name: 'schedules:schedulesPage.createTask', exact: true })
    .click()
  await createStarted
  assert.equal(
    await dialog
      .getByRole('button', { name: 'schedules:schedulesPage.cancel', exact: true })
      .isEnabled(),
    false,
  )
  assert.equal(
    await dialog
      .getByRole('button', { name: 'schedules:schedulesPage.closeDialog', exact: true })
      .isEnabled(),
    false,
  )
  await page.keyboard.press('Escape')
  assert.equal(await dialog.isVisible(), true)
  releaseCreate()
  createHold = null
  await dialog.waitFor({ state: 'hidden' })

  await page.goto(base + '/?page=channels')
  await button('channels:channelsPage.configureName channels:channelsPage.telegram').click()
  await page.getByLabel('channels:channelsPage.botToken').fill('synthetic-fixture-token')
  await button('channels:channelsPage.connect').click()
  await page.getByText('Rejected fixture credentials', { exact: true }).waitFor()
  failures.credentials = false
  await button('channels:channelsPage.retry').click()
  await page.getByLabel('channels:channelsPage.botToken').fill('synthetic-fixture-token')
  await button('channels:channelsPage.connect').click()
  await page.getByText('channels:channelsPage.online', { exact: true }).waitFor()
  assert.equal(await page.getByLabel('channels:channelsPage.botToken').count(), 0)
  assert.equal(evidence.credentials, 2)
  assert.ok(
    (await page.evaluate(() => window.evidence.notices)).some((message) =>
      message.includes('nameTwoWayConnectionEstablished'),
    ),
  )

  await page.goto(base + '/?page=skills')
  await page.getByRole('alert').filter({ hasText: 'Fixture skills unavailable' }).waitFor()
  assert.equal(await page.getByText('skills:skillsPage.noGlobalSkills', { exact: true }).count(), 0)
  failures.skillRead = false
  await button('skills:skillsPage.retry').click()
  const skillSwitch = page.getByRole('switch', {
    name: 'skills:skillsPage.toggleSkillName Fixture skill',
  })
  await skillSwitch.waitFor()
  await skillSwitch.click()
  await page.getByRole('alert').filter({ hasText: 'Fixture skill update failed' }).waitFor()
  assert.equal(await skillSwitch.getAttribute('aria-checked'), 'true')
  failures.skillWrite = false
  await skillSwitch.click()
  await page.waitForFunction(
    () => document.querySelector('[role="switch"]').getAttribute('aria-checked') === 'false',
  )
  assert.equal(await page.getByRole('alert').count(), 0)
  await button('skills:skillsPage.saveSettings').click()
  await page.getByRole('alert').filter({ hasText: 'Fixture skill reload failed' }).waitFor()
  failures.skillReload = false
  await button('skills:skillsPage.retry').click()
  await button('skills:skillsPage.saveSettings').click()
  await page.waitForFunction(() =>
    window.evidence.notices.includes('skills:skillsPage.skillSettingsSavedAndReloaded'),
  )
  assert.equal(await page.getByRole('alert').count(), 0)
  skills.skills = []
  await page.reload()
  await page.getByText('skills:skillsPage.noGlobalSkills', { exact: true }).waitFor()
  assert.equal(await page.getByRole('alert').count(), 0)
  assert.deepEqual(errors, [])
  console.log(
    'UX feature smoke passed: workflow schedule create/save/reload, schedule dialog focus/escape/pending, manual channel failure/retry/success, skill errors/retry/empty state.',
  )
} finally {
  releaseCreate?.()
  await browser?.close()
  server.closeAllConnections()
  await new Promise((resolve) => server.close(resolve))
}
