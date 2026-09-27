// 使用隔离 Runtime 和确定性图片响应验证工作流；可选缓存目录用于真实 WASM 离线验收。
import assert from 'node:assert/strict'
import { readFile, writeFile, mkdtemp, mkdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright-core'
import { createServer } from 'node:net'
import { SPRITE_ENGINE_CATALOG } from '../shared/sprite-engine-catalog.mjs'
import { decodeWorkflowBundle } from '../runtime/services/workflow-bundle-archive.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const development = process.argv.includes('--dev')
async function developmentPort() {
  const http = createServer()
  const hmr = createServer()
  const listen = (server, port) =>
    new Promise((resolve, reject) => {
      server.once('error', reject)
      server.listen(port, '127.0.0.1', () => resolve(server.address().port))
    })
  try {
    const port = await listen(http, 0)
    await listen(hmr, port + 1)
    return port
  } finally {
    await Promise.all(
      [http, hmr]
        .filter((server) => server.listening)
        .map((server) => new Promise((resolve) => server.close(resolve))),
    )
  }
}
const browserPath = process.env.PISPER_UI_BROWSER_PATH || chromium.executablePath()
const output = await mkdtemp(join(tmpdir(), 'pisper-workflows-ui-'))
const dataDir = join(output, 'agent')
const workspace = join(output, 'workspace')
await mkdir(workspace)
const fixtureHome = join(output, 'home')
await mkdir(fixtureHome)
process.env.HOME = fixtureHome
process.env.USERPROFILE = fixtureHome
process.env.PI_SKIP_VERSION_CHECK = '1'
process.env.PI_TELEMETRY = '0'
process.env.PISPER_AGENT_DIR = dataDir
process.env.PISPER_WORKSPACE_DIR = workspace
const { createPisperRuntime } = await import('../runtime/app-runtime.mjs')
const app = await createPisperRuntime({
  root,
  frontendRoot: join(root, 'dist'),
  runtimeCwd: workspace,
  dataDir,
  production: !development,
  port: development ? await developmentPort() : 0,
  host: '127.0.0.1',
  remote: { enabled: false },
})
await app.initialized
const runtime = app.runtime
const base = app.url
const report = { status: 'running', checks: [], errors: [], engineDownloads: 0 }
let browser, page
const api = async (path, method = 'GET', data) => {
  const response = await fetch(base + path, {
    method,
    headers: { 'Content-Type': 'application/json', Origin: base },
    body: data === undefined ? undefined : JSON.stringify(data),
  })
  assert.ok(
    response.ok,
    `${method} ${path}: ${response.status} ${response.ok ? '' : await response.text()}`,
  )
  return response.json()
}
try {
  const model = {
    id: 'paint',
    name: 'Fixture image model',
    providerId: 'fixture',
    providerName: 'Fixture',
  }
  runtime.visualGeneration.getModelStatus = () =>
    Promise.resolve({ models: [model], model, selection: 'fixture/paint' })
  runtime.spriteEngines.fetchFn = async (url) => {
    report.engineDownloads++
    const file = SPRITE_ENGINE_CATALOG.flatMap((engine) => engine.files).find((entry) =>
      [entry.url, ...(entry.fallbackUrls ?? [])].includes(String(url)),
    )
    assert.ok(file, 'Only fixed catalog resources may be downloaded')
    assert.ok(process.env.PISPER_TEST_SPRITE_ENGINE_DIR, 'An explicit engine cache is required')
    return new Response(await readFile(join(process.env.PISPER_TEST_SPRITE_ENGINE_DIR, file.name)))
  }
  const waitForCompletedRun = async (id, timeout = 60000) => {
    const deadline = Date.now() + timeout
    let run
    do {
      run = await api(`/api/workflow-runs/${encodeURIComponent(id)}`)
      if (!['running', 'waiting_approval'].includes(run.status)) break
      await new Promise((resolve) => setTimeout(resolve, 100))
    } while (Date.now() < deadline)
    assert.equal(
      run.status,
      'completed',
      `Workflow terminal status: ${run.status}, ${run.error ?? ''}`,
    )
    return run
  }
  const waitEngine = async (id) => {
    const deadline = Date.now() + 60000
    let engine
    do {
      engine = (await api('/api/sprite-engines')).engines.find((item) => item.id === id)
      if (engine.status !== 'downloading') break
      await new Promise((resolve) => setTimeout(resolve, 100))
    } while (Date.now() < deadline)
    assert.equal(engine.status, 'ready', `Engine ${id}: ${engine.status}`)
  }
  browser = await chromium.launch({ headless: true, executablePath: browserPath })
  page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, locale: 'zh-CN' })
  page.on('pageerror', (error) => report.errors.push(error.message))
  const oldProjectCalls = []
  page.on('request', (request) => {
    if (new URL(request.url()).pathname.startsWith('/api/sprite-workflows'))
      oldProjectCalls.push(request.method())
  })
  await page.route('**/api/providers/discovery', (route) =>
    route.fulfill({ json: { providers: [], errors: [] } }),
  )
  await page.route('**/api/providers/import-local', (route) =>
    route.fulfill({
      json: { providers: [], changes: [], errors: [], added: 0, updated: 0, skipped: 0 },
    }),
  )
  await page.goto(base + '/#/workflows')
  await page.getByRole('button', { name: '稍后再说', exact: true }).click()
  await page.getByRole('button', { name: '使用工作流', exact: true }).click()
  await page.waitForURL((url) => url.hash === '#/workflows/new?template=sprite')
  await page.locator('.react-flow__node').first().waitFor()
  assert.equal(await page.locator('.react-flow__node').count(), 20)
  assert.equal(await page.locator('.react-flow__edge').count(), 22)
  assert.equal(await page.getByRole('tab', { name: '精灵工作台', exact: true }).count(), 0)
  assert.equal(await page.getByRole('region', { name: '游戏精灵图生成', exact: true }).count(), 0)
  assert.equal(
    await page.getByRole('main').getByRole('button', { name: '新建项目', exact: true }).count(),
    0,
  )
  const dismissInspector = async () => {
    const sheet = page.getByRole('dialog', { name: '节点属性', exact: true })
    if (await sheet.isVisible()) {
      await page.keyboard.press('Escape')
      await sheet.waitFor({ state: 'hidden' })
    }
  }
  const openWorkflowSettings = async () => {
    await dismissInspector()
    await page.getByRole('button', { name: '工作流设置', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: '工作流设置', exact: true })
    await dialog.waitFor()
    return dialog
  }
  for (const width of [1300, 1000, 760, 390]) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 })
    await page.waitForFunction(() => {
      const editor = document.querySelector('.workflow-editor-page')
      return editor && Boolean(editor.querySelector('aside')) === editor.clientWidth >= 1000
    })
    await dismissInspector()
    const layout = await page.locator('.builder-layout').evaluate((element) => {
      const editor = element.closest('.workflow-editor-page')
      const canvas = element.querySelector('.builder-canvas')
      const aside = element.querySelector('aside')
      const box = (node) => node?.getBoundingClientRect().toJSON()
      return {
        width: editor.clientWidth,
        main: box(element),
        canvas: box(canvas),
        aside: box(aside),
      }
    })
    assert.ok(
      layout.canvas.width >= layout.main.width - (layout.aside ? 322 : 2),
      'Canvas uses all space outside the optional inspector',
    )
    assert.equal(Boolean(layout.aside), layout.width >= 1000)
    if (layout.aside) {
      assert.ok(Math.abs(layout.aside.top - layout.canvas.top) < 2)
      assert.ok(layout.aside.left >= layout.canvas.right - 1, 'Inspector does not overlap canvas')
    }
    assert.equal(
      await page.getByRole('main').getByLabel('名称', { exact: true }).count(),
      0,
      'Workflow forms are not rendered below the canvas',
    )
    assert.ok(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1),
      `No page overflow at ${width}px`,
    )
    await page.locator('.react-flow__controls-fitview').click()
    await page.locator('.react-flow__node').first().click()
    const inspector =
      layout.width < 1000
        ? page.getByRole('dialog', { name: '节点属性', exact: true })
        : page.getByRole('complementary', { name: '节点属性', exact: true })
    await inspector.waitFor()
    await inspector.getByLabel('节点名称', { exact: true }).scrollIntoViewIfNeeded()
    assert.equal(
      await inspector.getByLabel('节点模型', { exact: true }).count(),
      0,
      'Manual triggers have no model selector',
    )
    assert.equal(
      await inspector.getByRole('spinbutton').count(),
      0,
      'Manual triggers have no retries or timeouts',
    )
    await page.screenshot({ path: join(output, `workflow-layout-${width}.png`), fullPage: true })
    await dismissInspector()
    await page.locator('.react-flow__controls-fitview').click()
    const settings = await openWorkflowSettings()
    await settings.getByRole('heading', { name: '运行输入', exact: true }).waitFor()
    const editInput = settings.getByRole('button', { name: /^编辑输入：/ }).first()
    await editInput.scrollIntoViewIfNeeded()
    assert.ok(
      await editInput.evaluate((element) => {
        const rect = element.getBoundingClientRect()
        return element.contains(
          document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2),
        )
      }),
      'Scrolled input editor stays above the canvas',
    )
    assert.equal(await settings.getByLabel('变量名', { exact: true }).count(), 0)
    await editInput.click()
    await settings.getByText('高级设置', { exact: true }).click()
    await settings.getByLabel('变量名', { exact: true }).waitFor()
    await page.keyboard.press('Escape')
    await settings.waitFor({ state: 'hidden' })
    assert.ok(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1),
    )
  }
  report.checks.push(
    '1300/1000/760/390px editor uses canvas plus responsive inspector, no bottom form or overlap; trigger fields and settings input disclosure are accessible',
  )
  await page.setViewportSize({ width: 1440, height: 1000 })
  await page.reload()
  await page.locator('.react-flow__node').first().waitFor()
  await page.screenshot({ path: join(output, 'workflow-graph-desktop.png'), fullPage: true })
  const fixture = await page.evaluate(() => {
    const source = document.createElement('canvas')
    source.width = source.height = 256
    const context = source.getContext('2d')
    context.fillStyle = '#00ff00'
    context.fillRect(0, 0, 256, 256)
    context.fillStyle = '#3555aa'
    context.fillRect(80, 42, 96, 172)
    context.fillStyle = '#cc7733'
    context.fillRect(92, 20, 72, 60)
    const sheets = {}
    for (const background of ['#FF00FF', '#00FF00', '#00FFFF', '#FFFFFF', '#000000']) {
      const canvas = document.createElement('canvas')
      canvas.width = 512
      canvas.height = 128
      const ctx = canvas.getContext('2d')
      ctx.fillStyle = background
      ctx.fillRect(0, 0, canvas.width, canvas.height)
      for (let frame = 0; frame < 4; frame++) {
        const x = frame * 128 + 42
        ctx.fillStyle = '#3555aa'
        ctx.fillRect(x, 36 + (frame % 2) * 2, 44, 56)
        ctx.fillStyle = '#cc7733'
        ctx.fillRect(x + 8, 18, 28, 26)
        ctx.fillStyle = '#223344'
        ctx.fillRect(x + frame * 2, 88, 12, 24)
        ctx.fillRect(x + 26 - frame * 2, 88, 12, 24)
      }
      sheets[background] = canvas.toDataURL('image/png').split(',')[1]
    }
    return { source: source.toDataURL('image/png').split(',')[1], sheets }
  })
  const png = Buffer.from(fixture.source, 'base64')
  let generatedImages = 0
  runtime.workflowImageOperations.generateImage = async (request, { signal, allowFallback }) => {
    generatedImages++
    signal.throwIfAborted()
    assert.equal(allowFallback, false)
    assert.equal(request.sourceImages.length, 1)
    assert.match(request.prompt, /style/i)
    assert.match(request.prompt, /4-frame continuous/)
    assert.doesNotMatch(request.prompt, /8 rows|64.frame/)
    const color = request.prompt.match(/Solid (#[0-9a-f]{6}) background/i)?.[1].toUpperCase()
    assert.ok(color && fixture.sheets[color], 'Generation uses a deterministic contrasting color')
    const path = join(request.cwd, 'generated', 'visuals', request.outputName + '.png')
    await mkdir(dirname(path), { recursive: true })
    await writeFile(path, Buffer.from(fixture.sheets[color], 'base64'))
    return { path, mimeType: 'image/png' }
  }
  await page.getByRole('button', { name: '试运行', exact: true }).click()
  const runDialog = page.getByRole('dialog')
  await runDialog.getByText(/预计调用图像模型 32 次/).waitFor()
  await runDialog
    .getByRole('textbox', { name: /^本次任务/ })
    .fill('Preserve the original blue character and equipment.')
  const sourceUploaded = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === '/api/workflow-media' &&
      response.request().method() === 'POST',
  )
  await runDialog
    .getByLabel('选择图片', { exact: true })
    .setInputFiles({ name: 'hero.png', mimeType: 'image/png', buffer: png })
  const sourceResponse = await sourceUploaded
  assert.equal(sourceResponse.status(), 201)
  const reference = await sourceResponse.json()
  await runDialog.getByRole('img', { name: 'hero.png', exact: true }).waitFor()
  const startEvent = page.waitForResponse(
    (response) =>
      /\/api\/workflows\/[^/]+\/run$/.test(new URL(response.url()).pathname) &&
      response.request().method() === 'POST',
  )
  await runDialog.getByRole('button', { name: '运行', exact: true }).click()
  const start = await startEvent
  assert.equal(start.status(), 202)
  const started = await start.json()
  await page.waitForURL((url) => url.hash === `#/workflows/${started.run.workflowId}`)
  const spriteRun = await waitForCompletedRun(started.run.id)
  assert.equal(generatedImages, 32)
  const catalog = await api('/api/workflows')
  const workflow = catalog.workflows.find((item) => item.id === spriteRun.workflowId)
  const exportNode = workflow.nodes.find((node) => node.kind === 'media-export')
  const transformNode = workflow.nodes.find((node) => node.kind === 'media-transform')
  const generatedOutput = spriteRun.nodes.find((node) => node.id === exportNode.id).output
  assert.equal(generatedOutput.frames.length, 128)
  assert.equal(generatedOutput.atlas.frames.length, 128)
  assert.equal(new Set(generatedOutput.frames.map((frame) => frame.direction)).size, 8)
  assert.equal(new Set(generatedOutput.frames.map((frame) => frame.action)).size, 4)
  await page.reload()
  const selectNode = async (id) => {
    await page.locator('.react-flow__controls-fitview').click()
    const node = page.locator(`.react-flow__node[data-id="${id}"]`)
    await node.click()
    await page.getByRole('region', { name: '节点结果', exact: true }).waitFor()
  }
  await selectNode(exportNode.id)
  const result = page.getByRole('region', { name: '节点结果', exact: true })
  await result.getByRole('button', { name: '播放动画', exact: true }).click()
  await result.getByRole('button', { name: '暂停动画', exact: true }).click()
  await result.getByRole('switch', { name: '前后帧叠影（洋葱皮）', exact: true }).click()
  const jsonEvent = page.waitForEvent('download')
  await result.getByRole('button', { name: 'JSON', exact: true }).click()
  const metadataDownload = await jsonEvent
  const metadata = JSON.parse(await readFile(await metadataDownload.path(), 'utf8'))
  assert.equal(metadata.frames.length, 128)
  const pngEvent = page.waitForEvent('download')
  await result.getByRole('button', { name: 'PNG', exact: true }).click()
  const pngDownload = await pngEvent
  const atlas = await readFile(await pngDownload.path())
  assert.ok(atlas.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])))
  await page.screenshot({ path: join(output, 'workflow-sprite-desktop.png'), fullPage: true })
  assert.equal(
    report.engineDownloads,
    0,
    'Color-key nodes and opening the workflow must not download engines',
  )
  assert.deepEqual(oldProjectCalls, [], 'The workflow must not request independent sprite projects')
  report.checks.push(
    'ordinary sprite DAG, editable graph, image/text inputs, explicit 32-call estimate, 128 real PNG frames, animation and atlas/JSON exports',
  )

  await selectNode(transformNode.id)
  await page.getByRole('spinbutton', { name: '留白（像素）', exact: true }).fill('8')
  const nodeRunEvent = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname ===
        `/api/workflows/${workflow.id}/nodes/${transformNode.id}/run` &&
      response.request().method() === 'POST',
  )
  await page.getByRole('button', { name: '仅运行此节点', exact: true }).click()
  const nodeStart = await nodeRunEvent
  assert.equal(nodeStart.status(), 202)
  const nodeStarted = await nodeStart.json()
  const nodeRun = await waitForCompletedRun(nodeStarted.run.id)
  assert.equal(generatedImages, 32, 'Local frame adjustments must reuse paid upstream outputs')
  assert.ok(nodeRun.nodes.some((node) => node.reused))
  assert.ok(nodeRun.nodes.find((node) => node.id === transformNode.id).output.frames.length > 0)
  assert.ok(
    !nodeRun.nodes.some((node) => node.id === exportNode.id),
    'Downstream atlas is invalidated after editing its input',
  )
  for (const downstream of [
    workflow.nodes.find((node) => node.kind === 'media-preview'),
    exportNode,
  ]) {
    await page.reload()
    await page.locator('.react-flow__controls-fitview').click()
    await page.locator(`.react-flow__node[data-id="${downstream.id}"]`).click()
    const event = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname ===
          `/api/workflows/${workflow.id}/nodes/${downstream.id}/run` &&
        response.request().method() === 'POST',
    )
    await page.getByRole('button', { name: '仅运行此节点', exact: true }).click()
    const response = await event
    assert.equal(response.status(), 202)
    const completed = await waitForCompletedRun((await response.json()).run.id)
    assert.equal(
      completed.nodes.find((node) => node.id === downstream.id).output.frames.length,
      128,
    )
    if (downstream.id === exportNode.id)
      assert.equal(
        completed.nodes.find((node) => node.id === downstream.id).output.atlas.frames.length,
        128,
      )
  }
  assert.equal(generatedImages, 32, 'Rebuilding preview and export must not regenerate paid images')
  assert.equal(
    new URL(page.url()).hash,
    `#/workflows/${workflow.id}`,
    'Saving and rerunning stay on the canvas',
  )
  await page.reload()
  await selectNode(transformNode.id)
  await page.setViewportSize({ width: 390, height: 844 })
  assert.ok(
    await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1),
  )
  // 响应式 Inspector 会重新挂载到 Sheet，先等待新的宿主，避免操作已卸载的侧栏。
  const mobileInspector = page.getByRole('dialog', { name: '节点属性', exact: true })
  await mobileInspector.waitFor({ state: 'visible' })
  await mobileInspector.getByRole('spinbutton', { name: '留白（像素）', exact: true }).click()
  await page.screenshot({
    path: join(output, 'workflow-image-inspector-mobile.png'),
    fullPage: true,
  })
  await page
    .getByRole('region', { name: '节点结果', exact: true })
    .screenshot({ path: join(output, 'workflow-image-preview-mobile.png') })
  report.checks.push(
    'single-node local rerun reuses upstream results without more image calls; saved graph remains open; mobile inspector fits',
  )

  const updated = (await api('/api/workflows')).workflows.find((item) => item.id === workflow.id)
  await api(`/api/workflows/${workflow.id}`, 'PATCH', {
    inputs: updated.inputs.map((input) =>
      input.name === 'reference' ? { ...input, defaultValue: reference } : input,
    ),
  })
  const bundleResponse = await fetch(base + `/api/workflows/${workflow.id}/bundle`)
  assert.equal(bundleResponse.status, 200)
  const zip = Buffer.from(await bundleResponse.arrayBuffer())
  const entries = decodeWorkflowBundle(zip)
  const bundleWorkflow = JSON.parse(new TextDecoder().decode(entries['workflow.json'])).workflow
  assert.equal(bundleWorkflow.nodes.filter((node) => node.kind === 'media-generate').length, 4)
  assert.deepEqual(
    bundleWorkflow.nodes.find((node) => node.id === transformNode.id).image.padding,
    8,
  )
  assert.ok(entries[`media/${reference.id}/data.bin`])
  assert.equal(JSON.parse(new TextDecoder().decode(entries['manifest.json'])).kind, 'dag')
  await page.goto(base + '/#/workflows')
  const importEvent = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === '/api/workflows/import-bundle' &&
      response.request().method() === 'POST',
  )
  await page
    .getByRole('main')
    .locator('input[type="file"][accept*=".zip"]')
    .setInputFiles({ name: 'sprite-workflow.zip', mimeType: 'application/zip', buffer: zip })
  const importedResponse = await importEvent
  assert.equal(importedResponse.status(), 201)
  const importedWorkflow = (await importedResponse.json()).workflow
  assert.notEqual(importedWorkflow.id, workflow.id)
  assert.equal(importedWorkflow.nodes.length, workflow.nodes.length)
  const importedReference = importedWorkflow.inputs.find(
    (input) => input.name === 'reference',
  ).defaultValue
  assert.notEqual(importedReference.id, reference.id)
  assert.deepEqual((await runtime.workflowMedia.load(importedReference.id)).buffer, png)
  await page
    .getByRole('button', { name: `编辑: ${importedWorkflow.name}`, exact: true })
    .last()
    .waitFor()
  report.checks.push(
    'single workflow ZIP carries node settings and default media, UI import creates an independent reusable graph',
  )

  if (process.env.PISPER_TEST_SPRITE_ENGINE_DIR) {
    for (const id of ['background', 'inpaint']) {
      await api(`/api/sprite-engines/${id}/download`, 'POST', {})
      await waitEngine(id)
    }
    const { workflow: offlineWorkflow } = await api('/api/workflows', 'POST', {
      name: 'Offline algorithm fixture',
      inputs: [
        {
          id: 'reference',
          name: 'reference',
          label: 'Reference',
          type: 'image',
          required: true,
          defaultValue: reference,
        },
      ],
      nodes: [
        { id: 'source', kind: 'media-input' },
        { id: 'background', kind: 'media-background', image: { method: 'model' } },
        { id: 'inpaint', kind: 'media-inpaint' },
        { id: 'export', kind: 'media-export' },
      ],
      edges: [
        ['source', 'background'],
        ['background', 'inpaint'],
        ['inpaint', 'export'],
      ].map(([source, target], index) => ({
        id: `edge-${index}`,
        source,
        target,
        sourcePort: 'output',
        targetPort: 'input',
      })),
    })
    const bundle = await fetch(base + `/api/workflows/${offlineWorkflow.id}/bundle`)
    assert.equal(bundle.status, 200)
    const offlineZip = Buffer.from(await bundle.arrayBuffer())
    const offlineEntries = decodeWorkflowBundle(offlineZip)
    for (const engine of SPRITE_ENGINE_CATALOG)
      for (const file of engine.files)
        assert.ok(offlineEntries[`engines/${engine.id}/${file.name}`])
    await api('/api/sprite-engines/background', 'DELETE')
    await api('/api/sprite-engines/inpaint', 'DELETE')
    runtime.spriteEngines.fetchFn = () => Promise.reject(new Error('offline fixture'))
    const imported = await fetch(base + '/api/workflows/import-bundle', {
      method: 'POST',
      headers: { 'Content-Type': 'application/zip', Origin: base },
      body: offlineZip,
    })
    assert.equal(imported.status, 201)
    assert.ok(
      (await api('/api/sprite-engines')).engines.every((engine) => engine.status === 'ready'),
    )
    assert.equal(generatedImages, 32)
    report.checks.push(
      'optional catalog-verified engines and licenses use the same workflow ZIP, offline import restores both algorithms without network',
    )
  }

  // 素材完全由测试生成；隔离 Runtime 的确定性 Agent 不读取配置或调用真实模型。
  const workflowPrompts = []
  runtime.workflows.agent.prompt = (input) => {
    workflowPrompts.push(input)
    return Promise.resolve({
      text: 'Fixture media inspected',
      sessionId: 'fixture-workflow-media',
      assets: [],
    })
  }
  const { workflow: mediaWorkflow } = await api('/api/workflows', 'POST', {
    name: '媒体输入回归',
    inputs: [
      {
        id: 'task',
        name: 'task',
        label: '本次任务',
        type: 'text',
        required: true,
        defaultValue: '',
      },
      {
        id: 'image',
        name: 'image',
        label: '参考图',
        type: 'image',
        required: true,
        defaultValue: null,
      },
      {
        id: 'video',
        name: 'video',
        label: '参考视频',
        type: 'video',
        required: true,
        defaultValue: null,
      },
    ],
    nodes: [
      {
        id: 'inspect',
        kind: 'prompt',
        label: '检查素材',
        prompt: '按本次任务 {{inputs.task}} 检查图片 {{inputs.image}} 与视频 {{inputs.video}}。',
      },
    ],
  })
  await page.goto(base + '/#/workflows/' + encodeURIComponent(mediaWorkflow.id))
  await page.getByRole('button', { name: '试运行', exact: true }).click()
  const mediaDialog = page.getByRole('dialog')
  await mediaDialog.waitFor()
  const runButton = mediaDialog.getByRole('button', { name: '运行', exact: true })
  await mediaDialog.getByRole('textbox', { name: /^本次任务/ }).fill('UI media snapshot fixture')
  const webm = Buffer.from(
    await page.evaluate(async () => {
      if (!MediaRecorder.isTypeSupported('video/webm;codecs=vp8'))
        throw new Error('The workflow UI fixture requires WebM VP8 recording support')
      const canvas = document.createElement('canvas')
      canvas.width = 160
      canvas.height = 96
      const context = canvas.getContext('2d')
      if (!context) throw new Error('Canvas unavailable')
      const stream = canvas.captureStream(0)
      const track = stream.getVideoTracks()[0]
      const recorder = new MediaRecorder(stream, { mimeType: 'video/webm;codecs=vp8' })
      const chunks = []
      const recording = new Promise((resolve, reject) => {
        recorder.ondataavailable = (event) => {
          if (event.data.size) chunks.push(event.data)
        }
        recorder.onstop = () => resolve(new Blob(chunks, { type: 'video/webm' }))
        recorder.onerror = () => reject(new Error('Fixture video recording failed'))
      })
      try {
        recorder.start()
        // 固定六帧和帧间隔用于编码短片，并非等待上传或 UI 时序。
        for (let frame = 0; frame < 6; frame++) {
          context.fillStyle = '#223344'
          context.fillRect(0, 0, 160, 96)
          context.fillStyle = '#55cc88'
          context.fillRect(12 + frame * 16, 28, 24, 40)
          track.requestFrame()
          await new Promise((resolve) => setTimeout(resolve, 50))
        }
        recorder.stop()
        const result = await recording
        return Array.from(new Uint8Array(await result.arrayBuffer()))
      } finally {
        for (const item of stream.getTracks()) item.stop()
      }
    }),
  )
  assert.ok(webm.length > 8 && webm.length < 1024 * 1024, 'Fixture video is a small real WebM')

  const uploadMedia = async (label, file) => {
    const started = Promise.withResolvers()
    const release = Promise.withResolvers()
    const routePattern = '**/api/workflow-media?*'
    const holdUpload = async (route) => {
      if (route.request().method() === 'POST') {
        started.resolve()
        await release.promise
      }
      await route.continue()
    }
    await page.route(routePattern, holdUpload)
    const responseEvent = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === '/api/workflow-media' &&
        response.request().method() === 'POST',
    )
    try {
      await mediaDialog.getByLabel(label, { exact: true }).setInputFiles(file)
      await started.promise
      await mediaDialog.getByText('正在上传素材…', { exact: true }).waitFor()
      assert.equal(await runButton.isDisabled(), true, 'Run is disabled until the upload finishes')
      release.resolve()
      const response = await responseEvent
      assert.equal(response.status(), 201)
      const reference = await response.json()
      await mediaDialog.getByText('正在上传素材…', { exact: true }).waitFor({ state: 'hidden' })
      assert.equal(await runButton.isEnabled(), true)
      return reference
    } finally {
      release.resolve()
      await page.unroute(routePattern, holdUpload)
    }
  }
  const imageReference = await uploadMedia('选择图片', {
    name: 'reference.png',
    mimeType: 'image/png',
    buffer: png,
  })
  await mediaDialog.getByRole('img', { name: 'reference.png', exact: true }).waitFor()
  await page.waitForFunction(() => {
    const image = document.querySelector('[role="dialog"] img')
    return image instanceof HTMLImageElement && image.complete && image.naturalWidth === 256
  })
  const videoReference = await uploadMedia('选择视频', {
    name: 'reference.webm',
    mimeType: 'video/webm',
    buffer: webm,
  })
  await mediaDialog.locator('video').waitFor()
  await page.waitForFunction(() => {
    const video = document.querySelector('[role="dialog"] video')
    return (
      video instanceof HTMLVideoElement &&
      video.readyState >= 1 &&
      video.videoWidth === 160 &&
      video.videoHeight === 96
    )
  })
  await mediaDialog.locator('video').evaluate(async (video) => {
    video.muted = true
    await video.play()
  })
  await page.waitForFunction(() => document.querySelector('[role="dialog"] video')?.currentTime > 0)
  await mediaDialog.locator('video').evaluate((video) => video.pause())
  await page.screenshot({ path: join(output, 'workflow-media-inputs-mobile.png'), fullPage: true })
  const runResponseEvent = page.waitForResponse(
    (response) =>
      response.url().endsWith(`/api/workflows/${mediaWorkflow.id}/run`) &&
      response.request().method() === 'POST',
  )
  await runButton.click()
  const runResponse = await runResponseEvent
  assert.equal(runResponse.status(), 202)
  const expectedInputs = {
    task: 'UI media snapshot fixture',
    image: imageReference,
    video: videoReference,
  }
  assert.deepEqual(runResponse.request().postDataJSON(), { inputs: expectedInputs })
  const { run } = await runResponse.json()
  // 在 Node 侧轮询终态；浏览器 waitForFunction 不用于异步 HTTP 谓词。
  const deadline = Date.now() + 10000
  let completedRun
  do {
    completedRun = await api(`/api/workflow-runs/${encodeURIComponent(run.id)}`)
    if (!['running', 'waiting_approval'].includes(completedRun.status)) break
    await new Promise((resolve) => setTimeout(resolve, 100))
  } while (Date.now() < deadline)
  assert.equal(completedRun.status, 'completed')
  assert.deepEqual(completedRun.inputs, expectedInputs)
  assert.equal(workflowPrompts.length, 1)
  assert.match(workflowPrompts[0].message, /UI media snapshot fixture/)
  assert.match(workflowPrompts[0].message, /not a native video model attachment/)
  assert.equal(workflowPrompts[0].attachments.length, 1)
  assert.deepEqual(Buffer.from(workflowPrompts[0].attachments[0].data, 'base64'), png)
  assert.deepEqual((await runtime.workflowMedia.load(videoReference.id)).buffer, webm)
  const storedWorkflow = (await api('/api/workflows')).workflows.find(
    (item) => item.id === mediaWorkflow.id,
  )
  assert.deepEqual(
    storedWorkflow.inputs.map((input) => input.defaultValue),
    mediaWorkflow.inputs.map((input) => input.defaultValue),
  )
  assert.ok(!JSON.stringify(completedRun.inputs).includes('base64'))
  await mediaDialog.waitFor({ state: 'hidden' })
  report.checks.push(
    'ordinary workflow real PNG/WebM uploads, image decode and video playback, upload gates run, text/media snapshots and deterministic agent attachments',
  )
  await page.goto(base + '/#/workflows/new')
  await page.locator('.react-flow__node').filter({ hasText: '手动触发' }).waitFor()
  const beforeAdd = await page.locator('.react-flow__node').count()
  await page.getByRole('button', { name: '添加节点', exact: true }).click()
  await page.getByRole('textbox', { name: '搜索节点', exact: true }).fill('区域修补')
  await page.getByRole('button', { name: '区域修补', exact: true }).click()
  await page.waitForFunction(
    (count) => document.querySelectorAll('.react-flow__node').length === count + 1,
    beforeAdd,
  )
  await page.getByRole('textbox', { name: '搜索节点', exact: true }).waitFor({ state: 'hidden' })
  await page.getByRole('button', { name: '返回工作流', exact: true }).click()
  await page.waitForURL((url) => url.hash === '#/workflows')
  await page.getByRole('button', { name: '使用工作流', exact: true }).waitFor()
  report.checks.push(
    'Searchable node palette adds an inpaint node through the UI; explicit Back to workflows returns to the list',
  )
  assert.deepEqual(report.errors, [])
  report.status = 'passed'
} catch (error) {
  report.status = 'failed'
  report.failure = String(error)
  if (page && !page.isClosed()) {
    await page.screenshot({ path: join(output, 'failure.png'), fullPage: true })
    console.log((await page.locator('body').ariaSnapshot()).slice(-10000))
  }
  throw error
} finally {
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
  await browser?.close()
  await app.close()
  await rm(dataDir, { recursive: true, force: true })
  console.log(JSON.stringify(report, null, 2))
  console.log(`Workflow UI report: ${output}`)
}
