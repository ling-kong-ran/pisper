// 通过真实沙箱桥验证游戏素材工作台。Runtime、素材和模型响应均隔离，不读取个人配置。
import assert from 'node:assert/strict'
import { createServer } from 'node:net'
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright-core'
import { PNG } from 'pngjs'
import { SPRITE_ENGINE_CATALOG } from '../shared/sprite-engine-catalog.mjs'

assert.equal(process.versions.node.split('.')[0], '24', 'Use the project Node.js 24 baseline')
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const development = process.argv.includes('--dev')
const browserPath = process.env.PISPER_UI_BROWSER_PATH || chromium.executablePath()
async function developmentPort() {
  const servers = [createServer(), createServer()]
  const listen = (server, port) =>
    new Promise((resolve, reject) => {
      server.once('error', reject)
      server.listen(port, '127.0.0.1', () => resolve(server.address().port))
    })
  try {
    const port = await listen(servers[0], 0)
    await listen(servers[1], port + 1)
    return port
  } finally {
    await Promise.all(
      servers
        .filter((server) => server.listening)
        .map((server) => new Promise((resolve) => server.close(resolve))),
    )
  }
}
function sheet(background = '#FFFFFF') {
  const image = new PNG({ width: 64, height: 16 })
  const color = [1, 3, 5].map((offset) => Number.parseInt(background.slice(offset, offset + 2), 16))
  for (let y = 0; y < 16; y++)
    for (let x = 0; x < 64; x++) {
      const index = (y * 64 + x) * 4
      const frame = Math.floor(x / 16)
      const foreground = x % 16 >= 5 && x % 16 <= 10 && y >= 3 + (frame % 2) && y <= 12
      image.data.set(foreground ? [40, 70, 140, 255] : [...color, 255], index)
    }
  return PNG.sync.write(image)
}
const output = await mkdtemp(join(tmpdir(), 'pisper-game-workbench-'))
const dataDir = join(output, 'agent'),
  workspace = join(output, 'workspace'),
  fixtureHome = join(output, 'home')
await Promise.all([mkdir(workspace), mkdir(fixtureHome)])
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
const runtime = app.runtime,
  base = app.url
const report = {
  status: 'running',
  checks: [],
  errors: [],
  generatedImages: 0,
  processingOperations: [],
  engineDownloads: 0,
}
let browser,
  page,
  releaseGeneration = null,
  releaseExport = null,
  releaseStaleCatalog = null
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
const waitRun = async (id) => {
  const deadline = Date.now() + 45000
  let run
  do {
    run = await api(`/api/game-assets/jobs/${encodeURIComponent(id)}`)
    if (!['running'].includes(run.status)) break
    await new Promise((resolve) => setTimeout(resolve, 100))
  } while (Date.now() < deadline)
  assert.equal(run.status, 'completed', `Expected completion: ${run.status}, ${run.error || ''}`)
  return run
}
const projectResponse = (response) =>
  new URL(response.url()).pathname === '/api/game-assets/projects' &&
  response.request().method() === 'POST'
try {
  assert.equal(
    (await runtime.toolPlugins.getState()).enabledTools.includes('image_assets'),
    false,
    'Agent image tools start disabled',
  )
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
    assert.ok(
      process.env.PISPER_TEST_SPRITE_ENGINE_DIR,
      'Algorithms require an explicit test cache',
    )
    const file = SPRITE_ENGINE_CATALOG.flatMap((engine) => engine.files).find((entry) =>
      [entry.url, ...(entry.fallbackUrls ?? [])].includes(String(url)),
    )
    assert.ok(file, 'Only catalog-verified resources may be installed')
    return new Response(await readFile(join(process.env.PISPER_TEST_SPRITE_ENGINE_DIR, file.name)))
  }
  const executeImage = runtime.gameAssetImageOperations.execute.bind(
    runtime.gameAssetImageOperations,
  )
  runtime.gameAssetImageOperations.execute = async (context) => {
    report.processingOperations.push(context.operation)
    if (context.operation === 'export' && releaseExport) await releaseExport.promise
    return executeImage(context)
  }
  runtime.gameAssetImageOperations.generateImage = async (request, { signal, allowFallback }) => {
    report.generatedImages++
    signal.throwIfAborted()
    assert.equal(allowFallback, false)
    assert.equal(request.sourceImages.length, 1)
    assert.match(request.prompt, /4-frame continuous/)
    const background = request.prompt.match(/Solid (#[0-9a-f]{6}) background/i)?.[1]
    assert.ok(background, 'Generated sheet uses an explicit contrasting background')
    if (releaseGeneration) await releaseGeneration.promise
    signal.throwIfAborted()
    const path = join(request.cwd, 'generated', 'visuals', request.outputName + '.png')
    await mkdir(dirname(path), { recursive: true })
    await writeFile(path, sheet(background))
    return { path, mimeType: 'image/png' }
  }
  browser = await chromium.launch({ headless: true, executablePath: browserPath })
  page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, locale: 'zh-CN' })
  page.on('pageerror', (error) => report.errors.push(error.message))
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
  await page.getByRole('button', { name: '更多工具', exact: true }).click()
  await page.getByRole('menuitem', { name: '游戏素材工作台', exact: true }).click()
  await page.waitForURL((url) => url.hash === '#/tools/game-assets')
  const workflowRequests = []
  page.on('request', (request) => {
    if (/\/api\/workflow/.test(new URL(request.url()).pathname))
      workflowRequests.push(new URL(request.url()).pathname)
  })
  async function workflowFiles() {
    try {
      return await readFile(runtime.workflows.path, 'utf8')
    } catch (error) {
      if (error.code === 'ENOENT') return null
      throw error
    }
  }
  const beforeWorkflowFiles = await workflowFiles()
  const widget = page.frameLocator('iframe[title="游戏素材工作台"]')
  await widget.getByRole('button', { name: '保存项目', exact: true }).waitFor()
  await widget.getByLabel('项目名称', { exact: true }).fill('Workbench empty draft')
  const emptySave = page.waitForResponse(projectResponse)
  await widget.getByRole('button', { name: '保存项目', exact: true }).click()
  const emptyResponse = await emptySave
  assert.equal(emptyResponse.status(), 201)
  const emptyProject = await emptyResponse.json()
  assert.equal(emptyProject.reference, null)
  await widget.getByText('项目已保存。', { exact: true }).waitFor()
  report.checks.push(
    'More tools opens the real sandbox component; empty image draft saves as an independent project',
  )

  await widget.getByRole('button', { name: '新建项目', exact: true }).click()
  await widget.getByLabel('项目名称', { exact: true }).fill('Game asset fixture')
  await widget
    .getByLabel('风格与动作要求', { exact: true })
    .fill('Preserve the original character style.')
  await widget
    .getByRole('combobox', { name: '图像模型', exact: true })
    .selectOption('fixture/paint')
  const directionInputs = widget.locator('#directions input[type="checkbox"]')
  for (let index = 1; index < (await directionInputs.count()); index++)
    await directionInputs.nth(index).uncheck()
  const actionInputs = widget.locator('#actions input[type="checkbox"]')
  for (let index = 1; index < (await actionInputs.count()); index++)
    await actionInputs.nth(index).uncheck()
  const uploadResponse = () =>
    page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === '/api/game-assets/media' &&
        response.request().method() === 'POST',
    )
  const uploadZone = widget.getByTestId('asset-upload-zone')
  const uploadInput = widget.locator('#upload')
  const imageFixture = { name: 'character.png', mimeType: 'image/png', buffer: sheet() }
  await page.setViewportSize({ width: 390, height: 844 })
  await uploadZone.scrollIntoViewIfNeeded()
  assert.equal(await uploadZone.getAttribute('role'), 'button')
  assert.equal(await uploadZone.getAttribute('aria-label'), '选择图片')
  assert.ok(
    await widget.locator('html').evaluate((html) => html.scrollWidth <= innerWidth + 1),
    'The empty workbench must fit the mobile iframe viewport',
  )
  assert.ok(
    await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
    'The empty workbench must not widen the app viewport',
  )
  await uploadZone.focus()
  assert.ok(await uploadZone.evaluate((zone) => document.activeElement === zone))
  await uploadZone.scrollIntoViewIfNeeded()
  const emptyUploadBounds = await uploadZone.boundingBox()
  assert.ok(
    emptyUploadBounds &&
      emptyUploadBounds.y >= 0 &&
      emptyUploadBounds.y + emptyUploadBounds.height <= 844,
    'The empty upload control must be visible in the mobile viewport',
  )
  await page.screenshot({ path: join(output, 'workbench-upload-mobile-empty.png'), fullPage: true })
  const keyboardChooser = page.waitForEvent('filechooser')
  const keyboardUpload = uploadResponse()
  await uploadZone.press('Enter')
  const selectedKeyboardChooser = await keyboardChooser
  assert.equal(await selectedKeyboardChooser.element().getAttribute('id'), 'upload')
  await selectedKeyboardChooser.setFiles(imageFixture)
  const uploadedResponse = await keyboardUpload
  assert.equal(uploadedResponse.status(), 201)
  const keyboardReference = await uploadedResponse.json()
  await widget.getByRole('img', { name: '角色图片', exact: true }).waitFor()
  await uploadZone.getByText('character.png', { exact: false }).waitFor()
  await widget.locator('#upload:not(:disabled)').waitFor()
  assert.equal(await uploadZone.getAttribute('aria-label'), '更换图片')
  const uploadHit = await uploadZone.evaluate((zone) => {
    const bounds = zone.getBoundingClientRect()
    const target = document.elementFromPoint(
      bounds.left + bounds.width / 2,
      bounds.top + bounds.height / 2,
    )
    return {
      withinZone: target === zone || zone.contains(target),
      isInput: target?.id === 'upload',
    }
  })
  assert.equal(uploadHit.withinZone, true, 'The visible upload control must receive pointer hits')
  assert.equal(
    uploadHit.isInput,
    false,
    'The hidden native input must not cover the upload control',
  )
  assert.ok(
    await widget.locator('html').evaluate((html) => html.scrollWidth <= innerWidth + 1),
    'The selected-image workbench must fit the mobile iframe viewport',
  )
  await page.screenshot({
    path: join(output, 'workbench-upload-mobile-selected.png'),
    fullPage: true,
  })
  const mouseChooser = page.waitForEvent('filechooser')
  const mouseUpload = uploadResponse()
  await uploadZone.click()
  const selectedMouseChooser = await mouseChooser
  assert.equal(await selectedMouseChooser.element().getAttribute('id'), 'upload')
  await selectedMouseChooser.setFiles(imageFixture)
  const mouseResponse = await mouseUpload
  assert.equal(mouseResponse.status(), 201)
  const mouseReference = await mouseResponse.json()
  assert.notEqual(
    mouseReference.id,
    keyboardReference.id,
    'Choosing the same file again must upload a fresh original',
  )
  await widget.locator('#upload:not(:disabled)').waitFor()
  const directUpload = uploadResponse()
  await uploadInput.setInputFiles(imageFixture)
  const directResponse = await directUpload
  assert.equal(directResponse.status(), 201)
  const uploadedReference = await directResponse.json()
  assert.notEqual(uploadedReference.id, mouseReference.id)
  await widget.locator('#upload:not(:disabled)').waitFor()
  await page.setViewportSize({ width: 1440, height: 1000 })
  report.checks.push(
    'The empty upload control opens a chooser with Enter; mouse selection and direct input upload replace the same image; mobile empty and selected states fit without an input overlay',
  )
  assert.equal(await widget.locator('#background-mode').inputValue(), 'model')
  assert.equal(
    await widget.getByRole('button', { name: '本地去背景', exact: true }).isDisabled(),
    true,
  )
  await widget
    .locator('#background-engine')
    .getByRole('button', { name: /下载算法 · [\d.,]+ MB/ })
    .waitFor()
  await widget
    .getByText('尚未安装此本地算法，请先点击下方按钮下载；不会自动下载。', { exact: false })
    .first()
    .waitFor()
  await widget.locator('#background').dispatchEvent('click')
  await widget
    .locator('#message')
    .getByText('尚未安装此本地算法，请先点击下方按钮下载；不会自动下载。', { exact: true })
    .waitFor()
  assert.deepEqual(
    report.processingOperations,
    [],
    'Missing default model cannot silently fall back to color processing',
  )
  assert.equal(report.engineDownloads, 0, 'Missing model is never automatically downloaded')
  await widget.locator('#background-mode').scrollIntoViewIfNeeded()
  await page.screenshot({ path: join(output, 'background-default.png'), fullPage: true })
  await widget.getByRole('combobox', { name: '去背景方式', exact: true }).selectOption('color')
  await widget
    .getByText('按颜色移除仅适合纯色背景。主体与背景同色时，可能误删主体。', { exact: true })
    .waitFor()
  report.checks.push(
    'Default local foreground model shows a visible sized download and cannot run while missing; color removal requires explicit selection',
  )
  const localEvent = page.waitForResponse(
    (response) => new URL(response.url()).pathname === '/api/game-assets/process',
  )
  await widget.getByRole('button', { name: '本地去背景', exact: true }).click()
  const localResponse = await localEvent
  assert.equal(localResponse.status(), 200)
  const processedReference = await localResponse.json()
  const processed = PNG.sync.read((await runtime.gameAssetMedia.load(processedReference.id)).buffer)
  assert.equal(processed.data[3], 0, 'Local color processing makes the corner transparent')
  assert.equal(
    processed.data[(8 * processed.width + 8) * 4 + 3],
    255,
    'Local processing preserves the character',
  )
  assert.equal(report.generatedImages, 0, 'Background processing never calls an image model')
  assert.equal(
    report.engineDownloads,
    0,
    'Local color processing never downloads optional algorithms',
  )
  const repeatResponse = page.waitForResponse(
    (response) => new URL(response.url()).pathname === '/api/game-assets/process',
  )
  await widget.getByRole('button', { name: '本地去背景', exact: true }).click()
  const repeated = await repeatResponse
  assert.equal(
    repeated.request().postDataJSON().reference.id,
    uploadedReference.id,
    'Repeated processing starts from the uploaded original, not its already processed result',
  )
  assert.equal(repeated.request().postDataJSON().image.method, 'color')
  await widget.getByRole('button', { name: '恢复原图', exact: true }).click()
  if (process.env.PISPER_TEST_SPRITE_ENGINE_DIR) {
    await widget.getByRole('combobox', { name: '去背景方式', exact: true }).selectOption('model')
    await widget
      .locator('#background-engine')
      .getByRole('button', { name: /下载算法 · [\d.,]+ MB/ })
      .click()
    await widget.locator('#background:not(:disabled)').waitFor()
    const modelResponse = page.waitForResponse(
      (response) => new URL(response.url()).pathname === '/api/game-assets/process',
    )
    await widget.getByRole('button', { name: '本地去背景', exact: true }).click()
    const modelProcessed = await modelResponse
    assert.equal(modelProcessed.status(), 200)
    assert.equal(modelProcessed.request().postDataJSON().image.method, 'model')
    assert.equal(modelProcessed.request().postDataJSON().reference.id, uploadedReference.id)
    const media = await modelProcessed.json()
    assert.ok(PNG.sync.read((await runtime.gameAssetMedia.load(media.id)).buffer).width > 0)
    assert.equal(
      report.generatedImages,
      0,
      'Local model background removal never calls paid image generation',
    )
    await widget.getByRole('button', { name: '恢复原图', exact: true }).click()
    report.checks.push(
      'Explicit catalog-verified download enables actual local U2Net WASM processing with no paid image request',
    )
  }
  releaseExport = Promise.withResolvers()
  const startEvent = page.waitForResponse(
    (response) =>
      /\/api\/game-assets\/projects\/[^/]+\/run$/.test(new URL(response.url()).pathname) &&
      response.request().method() === 'POST',
  )
  await widget.getByRole('button', { name: '保存并生成', exact: true }).click()
  const startResponse = await startEvent
  assert.equal(startResponse.status(), 202)
  const started = await startResponse.json()
  await widget.getByRole('button', { name: '播放', exact: true }).click()
  await widget.getByRole('button', { name: '暂停', exact: true }).click()
  assert.equal(
    await widget.getByRole('button', { name: '导出 PNG 图集', exact: true }).isDisabled(),
    true,
  )
  assert.equal((await api('/api/game-assets/jobs/' + started.job.id)).status, 'running')
  assert.match(await widget.locator('#counter').textContent(), / \/ 4$/)
  releaseExport.resolve()
  releaseExport = null
  const completed = await waitRun(started.job.id)
  assert.equal(report.generatedImages, 1)
  const exported = completed.output
  assert.equal(exported.frames.length, 4)
  assert.equal(exported.atlas.frames.length, 4)
  await widget.getByText('已完成', { exact: true }).waitFor()
  await widget.getByRole('button', { name: '播放', exact: true }).click()
  await widget.getByRole('button', { name: '暂停', exact: true }).click()
  const currentFrame = Number((await widget.locator('#counter').textContent()).split(' / ')[0])
  await widget.getByRole('button', { name: '下一帧', exact: true }).click()
  assert.equal(await widget.locator('#counter').textContent(), (currentFrame % 4) + 1 + ' / 4')
  for (const [label, format] of [
    ['导出 PNG 图集', 'png'],
    ['导出帧信息', 'json'],
  ]) {
    const downloadEvent = page.waitForEvent('download')
    await widget.getByRole('button', { name: label, exact: true }).click()
    const downloaded = await downloadEvent
    const bytes = await readFile(await downloaded.path())
    if (format === 'png') assert.ok(PNG.sync.read(bytes).width > 0)
    else assert.equal(JSON.parse(bytes.toString('utf8')).frames.length, 4)
  }
  await widget.locator('body').evaluate(() => window.scrollTo(0, 0))
  await page.screenshot({ path: join(output, 'workbench-desktop.png'), fullPage: true })
  report.checks.push(
    'Real bridge upload and local color processing; one controlled image call produces four preview frames before the atlas completes, plus PNG/JSON host downloads',
  )

  const beforeFrame = await runtime.gameAssetMedia.load(completed.output.frames[0].media.id)
  const editor = widget.locator('#frame-editor')
  await editor.getByText('逐帧编辑', { exact: true }).click()
  await editor.getByLabel('水平偏移', { exact: true }).fill('3')
  await editor.getByLabel('水平偏移', { exact: true }).blur()
  await editor.getByRole('button', { name: '复制帧', exact: true }).click()
  assert.equal(await editor.locator('#edit-info').textContent(), '2 / 5')
  await editor.getByRole('button', { name: '向后移动', exact: true }).click()
  assert.equal(await editor.locator('#edit-info').textContent(), '3 / 5')
  await editor.getByRole('button', { name: '删除帧', exact: true }).click()
  assert.equal(await editor.locator('#edit-info').textContent(), '3 / 4')
  await editor.getByRole('button', { name: '撤销', exact: true }).click()
  assert.equal(await editor.locator('#edit-info').textContent(), '3 / 5')
  await editor.getByRole('button', { name: '重做', exact: true }).click()
  assert.equal(await editor.locator('#edit-info').textContent(), '3 / 4')
  await editor.getByRole('button', { name: '帧 1', exact: true }).click()
  await editor.getByLabel('洋葱皮', { exact: true }).check()
  await editor.getByRole('button', { name: '擦除', exact: true }).click()
  const editCanvas = editor.locator('#edit-canvas')
  const box = await editCanvas.boundingBox()
  assert.ok(box)
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2)
  await page.mouse.down()
  await page.mouse.move(box.x + box.width * 0.65, box.y + box.height * 0.65, { steps: 5 })
  await page.mouse.up()
  await editor.getByRole('button', { name: '恢复', exact: true }).click()
  await editCanvas.click({ position: { x: box.width / 2, y: box.height / 2 } })
  const previewAlpha = await editCanvas.evaluate(async (canvas) => {
    // 等当前指针事件产生的绘制进入下一次合成帧，不用任意延时掩盖竞态。
    await new Promise(requestAnimationFrame)
    return Array.from(
      canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data,
    ).filter((_, index) => index % 4 === 3)
  })
  // 模拟编辑期间开始的旧列表请求，在编辑保存后才送达浏览器。
  const capturedCatalog = Promise.withResolvers()
  releaseStaleCatalog = Promise.withResolvers()
  let captured = false
  const holdStaleCatalog = async (route) => {
    if (route.request().method() !== 'GET' || captured) {
      await route.continue()
      return
    }
    captured = true
    const response = await route.fetch()
    const catalog = await response.json()
    capturedCatalog.resolve({
      route,
      response,
      revision: catalog.jobs.find((job) => job.id === completed.id)?.revision,
    })
    await releaseStaleCatalog.promise
    await route.fulfill({ response })
  }
  await page.route('**/api/game-assets', holdStaleCatalog)
  await widget.getByRole('button', { name: '刷新', exact: true }).click()
  const staleCatalog = await capturedCatalog.promise
  assert.equal(staleCatalog.revision, 0, 'The held list must predate the first frame edit')
  const brushResponse = page.waitForResponse((response) =>
    /\/api\/game-assets\/jobs\/[^/]+\/frames$/.test(new URL(response.url()).pathname),
  )
  await editor.getByRole('button', { name: '应用帧修改', exact: true }).click()
  const brushed = await (await brushResponse).json()
  assert.equal(brushed.revision, 1)
  const brushedPixels = PNG.sync.read(
    (await runtime.gameAssetMedia.load(brushed.output.frames[0].media.id)).buffer,
  )
  assert.deepEqual(
    previewAlpha,
    Array.from(brushedPixels.data).filter((_, index) => index % 4 === 3),
    'Fine diagonal brush preview alpha exactly matches the saved worker pixels',
  )
  await editor.getByText('帧修改已保存。', { exact: true }).waitFor()
  const staleResponse = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === '/api/game-assets' &&
      response.request().method() === 'GET',
  )
  releaseStaleCatalog.resolve()
  releaseStaleCatalog = null
  await staleResponse
  await widget.locator('body').evaluate(() => new Promise(requestAnimationFrame))
  await page.unroute('**/api/game-assets', holdStaleCatalog)
  assert.equal(
    await editor.getByLabel('水平偏移', { exact: true }).inputValue(),
    '3',
    'A stale list response must not replace newer saved frame edits',
  )
  report.checks.push(
    'Preview and saved brush masks match for every pixel, and a stale catalog revision cannot overwrite saved frame edits',
  )
  for (const [label, value] of [
    ['缩放', '0.8'],
    ['旋转（°）', '15'],
    ['透明度', '0.8'],
    ['帧时长（毫秒）', '240'],
  ]) {
    await editor.getByLabel(label, { exact: true }).fill(value)
    await editor.getByLabel(label, { exact: true }).blur()
  }
  const editsResponse = page.waitForResponse((response) =>
    /\/api\/game-assets\/jobs\/[^/]+\/frames$/.test(new URL(response.url()).pathname),
  )
  await editor.getByRole('button', { name: '应用帧修改', exact: true }).click()
  const edited = await (await editsResponse).json()
  assert.equal(edited.edits.frames[0].x, 3)
  assert.equal(edited.edits.frames[0].eraseStrokes.length, 2)
  assert.equal(edited.edits.frames[0].eraseStrokes[1].restore, true)
  assert.equal(edited.edits.frames[0].durationMs, 240)
  assert.equal(edited.edits.frames[0].rotation, 15)
  assert.equal(edited.edits.frames[0].scale, 0.8)
  assert.equal(edited.edits.frames[0].opacity, 0.8)
  assert.equal(report.generatedImages, 1, 'Manual edits do not call the image model')
  assert.notDeepEqual(
    (await runtime.gameAssetMedia.load(edited.output.frames[0].media.id)).buffer,
    beforeFrame.buffer,
  )
  await editor.getByText('帧修改已保存。', { exact: true }).waitFor()
  await page.screenshot({ path: join(output, 'frame-editor-desktop.png'), fullPage: true })
  report.checks.push(
    'Manual move, onion skin, erase, reorder, duplicate, delete, undo and redo; applying changes pixels locally without another model call',
  )

  await page.reload()
  await widget.locator('#project').selectOption(started.job.projectId)
  await widget.getByText('已完成', { exact: true }).waitFor()
  await editor.getByText('逐帧编辑', { exact: true }).click()
  assert.equal(await editor.getByLabel('水平偏移', { exact: true }).inputValue(), '3')
  await editor.getByRole('button', { name: '重置为原始帧', exact: true }).click()
  assert.equal(await editor.getByLabel('水平偏移', { exact: true }).inputValue(), '0')
  // 滚动到 iframe 深处后从键盘激活，也覆盖按钮的无障碍操作路径。
  await editor.getByRole('button', { name: '放弃未保存修改', exact: true }).press('Enter')
  assert.equal(await editor.getByLabel('水平偏移', { exact: true }).inputValue(), '3')
  await page.setViewportSize({ width: 390, height: 844 })
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
  assert.ok(
    await widget.locator('html').evaluate((html) => html.scrollWidth <= window.innerWidth + 1),
  )
  async function mobileTapTarget(locator, name) {
    await locator.scrollIntoViewIfNeeded()
    await locator.click({ trial: true })
    const bounds = await locator.boundingBox()
    assert.ok(bounds, `${name} must have visible bounds`)
    assert.ok(
      bounds.x >= -1 &&
        bounds.x + bounds.width <= 391 &&
        bounds.y >= -1 &&
        bounds.y + bounds.height <= 845,
      `${name} must fit within the mobile viewport`,
    )
  }
  await mobileTapTarget(widget.getByRole('button', { name: '保存项目', exact: true }), 'Save')
  await widget
    .locator('body')
    .evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight))
  await mobileTapTarget(
    widget.getByRole('button', { name: '导出 PNG 图集', exact: true }),
    'PNG export',
  )
  await mobileTapTarget(
    widget.getByRole('button', { name: '导出帧信息', exact: true }),
    'Frame data export',
  )
  await page.screenshot({ path: join(output, 'workbench-mobile-downloads.png'), fullPage: true })
  await editor.getByLabel('帧时长（毫秒）', { exact: true }).fill('241')
  await editor.getByLabel('帧时长（毫秒）', { exact: true }).blur()
  await mobileTapTarget(
    editor.getByRole('button', { name: '应用帧修改', exact: true }),
    'Frame edits',
  )
  await page.screenshot({ path: join(output, 'workbench-mobile-actions.png'), fullPage: true })
  await editor.getByRole('button', { name: '放弃未保存修改', exact: true }).click()
  await editCanvas.scrollIntoViewIfNeeded()
  await page.screenshot({ path: join(output, 'workbench-mobile.png'), fullPage: true })
  await page.setViewportSize({ width: 1440, height: 1000 })
  report.checks.push(
    'Saved frame edits survive reopening; workbench and editor fit a 390px viewport, and bottom save, download and frame-edit controls remain clickable',
  )

  releaseGeneration = Promise.withResolvers()
  const nextStartEvent = page.waitForResponse(
    (response) =>
      /\/api\/game-assets\/projects\/[^/]+\/run$/.test(new URL(response.url()).pathname) &&
      response.request().method() === 'POST',
  )
  await widget.getByRole('button', { name: '保存并生成', exact: true }).click()
  const secondJob = (await (await nextStartEvent).json()).job
  await widget.getByRole('button', { name: '停止生成', exact: true }).waitFor()
  await page.goto(base + '/#/chat')
  assert.equal((await api('/api/game-assets/jobs/' + secondJob.id)).status, 'running')
  releaseGeneration.resolve()
  releaseGeneration = null
  await waitRun(secondJob.id)
  report.checks.push('Leaving the workbench does not stop its background job')
  assert.deepEqual(workflowRequests, [], 'Workbench never accesses workflow APIs')
  assert.deepEqual(
    await workflowFiles(),
    beforeWorkflowFiles,
    'Workbench never creates or changes workflow files',
  )

  await page.goto(base + '/#/config/interface?view=layout')
  await page.getByRole('button', { name: '游戏素材工作台', exact: true }).waitFor()
  await page.evaluate(() => {
    window.workbenchPreviewRequests = []
    window.addEventListener('message', (event) => {
      if (
        event.data?.pisperBridge === 1 &&
        String(event.data.method || '').startsWith('gameAssets.')
      )
        window.workbenchPreviewRequests.push(event.data.method)
    })
  })
  await page.clock.install()
  await page.getByRole('button', { name: '游戏素材工作台', exact: true }).click()
  await widget.getByText('布局预览，打开工具后即可使用。', { exact: true }).waitFor()
  assert.equal(
    await widget.getByRole('button', { name: '保存项目', exact: true }).isDisabled(),
    true,
  )
  await page.clock.runFor(5000)
  assert.deepEqual(await page.evaluate(() => window.workbenchPreviewRequests), [])
  report.checks.push(
    'Layout preview has no asset permissions, stays disabled and sends no asset bridge requests or recurring polls',
  )
  assert.deepEqual(report.errors, [])
  assert.equal(
    report.engineDownloads,
    process.env.PISPER_TEST_SPRITE_ENGINE_DIR
      ? SPRITE_ENGINE_CATALOG.find((engine) => engine.id === 'background').files.length
      : 0,
  )
  report.status = 'passed'
} catch (error) {
  report.status = 'failed'
  report.failure = String(error)
  if (page && !page.isClosed()) {
    await page.screenshot({ path: join(output, 'failure.png'), fullPage: true })
    console.log((await page.locator('body').ariaSnapshot()).slice(-8000))
    for (const frame of page.frames().filter((frame) => frame !== page.mainFrame()))
      console.log((await frame.locator('body').ariaSnapshot()).slice(-8000))
  }
  throw error
} finally {
  releaseGeneration?.resolve()
  releaseExport?.resolve()
  releaseStaleCatalog?.resolve()
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
  await browser?.close()
  await app.close()
  await rm(dataDir, { recursive: true, force: true })
  console.log(JSON.stringify(report, null, 2))
  console.log(`Game asset workbench report: ${output}`)
}
