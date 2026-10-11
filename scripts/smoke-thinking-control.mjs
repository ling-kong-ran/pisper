// 真实会话面板、命令 Hook、模型选择器和滑块；HTTP 仅由本机内存服务响应。
import assert from 'node:assert/strict'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { createServer } from 'node:http'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const translations = JSON.parse(readFileSync(join(root, 'src/locales/en-US/chat.json'), 'utf8'))
const mocks = {
  '@/app/i18n/use-i18n': String.raw`
    const labels=${JSON.stringify(translations)};
    const t=(key,values={})=>(labels[key.replace(/^chat:/,'')]||key).replace(/\{(\w+)\}/g,(_,key)=>String(values[key]??''));
    export function useI18n(){return {t,language:'en-US'}}`,
  '@/lib/storage/page-state-storage': 'export const pageStateStorage=localStorage',
  '@/lib/platform/pick-system-directory':
    'export async function pickSystemDirectory(){return null}',
  '@/lib/http/http': 'export class ApiError extends Error {}',
  '@/features/chat/api/chat-api': `
    async function json(path,method='GET',body){
      const response=await fetch(path,{method,headers:{'Content-Type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)})});
      const data=await response.json();
      if(method==='GET')window.evidence.completedReads++;
      if(!response.ok)throw new Error(data.error);
      return data;
    }
    export const chatApi={
      getThinkingLevel:id=>json('/api/sessions/'+id+'/thinking-level'),
      updateModel:(id,provider,model)=>json('/api/sessions/'+id+'/model','PUT',{provider,model}),
      setThinkingLevel:(id,level)=>json('/api/sessions/'+id+'/thinking-level','PUT',{level}),
      getLiveSession:async()=>({streaming:false}),
    }`,
  // 省略无关转录、附件、语音；能力和请求逻辑完全来自生产 Hook/面板/控件。
  '@/features/chat/components/runtime/FocusSession': `
    import {ModelThinkingControl} from '@/features/chat/components/runtime/FocusRuntimeControls';
    export function FocusSession(props){
      return <main style={{padding:'180px 20px 20px',display:'flex',justifyContent:'center'}}>
        <ModelThinkingControl model={props.model} models={props.availableModels}
          thinkingLevel={props.thinkingLevel} levels={props.availableThinkingLevels}
          status={props.thinkingStatus} message={props.thinkingMessage}
          onModelChange={props.onModelChange} onThinkingChange={props.onThinkingLevelChange}
          onRefreshThinking={props.onLoadThinkingLevel}
          modelDisabled={props.switchingModel} thinkingDisabled={props.switchingThinking||props.switchingModel}/>
      </main>
    }`,
}
const output = await build({
  entryPoints: [join(root, 'scripts/fixtures/thinking-control.jsx')],
  bundle: true,
  write: false,
  loader: { '.css': 'empty' },
  format: 'esm',
  platform: 'browser',
  jsx: 'automatic',
  outdir: 'fixture',
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
          loader: 'jsx',
          resolveDir: root,
        }))
      },
    },
  ],
})
const js = output.outputFiles.find((file) => file.path.endsWith('.js')).text
const css = readdirSync(join(root, 'dist/assets'))
  .filter((file) => file.endsWith('.css'))
  .map((file) => readFileSync(join(root, 'dist/assets', file), 'utf8'))
  .join('\n')
const reasonerLevels = ['off', 'minimal', 'low', 'medium', 'high']
let available = { fixed: ['off'], reasoner: [...reasonerLevels] }
let model = 'reasoner'
let thinking = 'medium'
let legacyModelResponse = false
let omitThinkingIdentity = false
let failNextRead = false
let nextGate = null
const pendingGates = new Set()
const reads = []
const writes = []

function state() {
  const levels = available[model]
  if (!levels.includes(thinking)) {
    const order = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']
    thinking =
      levels.find((level) => order.indexOf(level) >= order.indexOf(thinking)) || levels.at(-1)
  }
  return {
    id: 'thinking-fixture',
    ...(!omitThinkingIdentity && { model: `fixture/${model}` }),
    thinkingLevel: thinking,
    availableLevels: [...levels],
    availableThinkingLevels: [...levels],
    status: 'ok',
    thinkingStatus: 'supported',
  }
}

function holdNextRead() {
  let release
  const promise = new Promise((resolve) => {
    release = resolve
  })
  const gate = { promise, release, started: false }
  nextGate = gate
  pendingGates.add(gate)
  return gate
}

const server = createServer(async (request, response) => {
  const send = (status, value) => {
    response.writeHead(status, { 'Content-Type': 'application/json' })
    response.end(JSON.stringify(value))
  }
  if (request.url.startsWith('/api/')) {
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : null
    if (request.url.endsWith('/model') && request.method === 'PUT') {
      model = body.model
      thinking = 'medium'
      const updated = state()
      writes.push({ kind: 'model', model })
      return send(200, legacyModelResponse ? { ok: true } : { ok: true, ...updated })
    }
    if (request.url.endsWith('/thinking-level') && request.method === 'PUT') {
      if (!available[model].includes(body.level)) {
        writes.push({ kind: 'invalid-thinking', model, level: body.level })
        return send(400, { error: 'Unsupported fixture thinking level' })
      }
      thinking = body.level
      writes.push({ kind: 'thinking', model, level: thinking })
      return send(200, state())
    }
    if (request.url.endsWith('/thinking-level') && request.method === 'GET') {
      const snapshot = state()
      reads.push(snapshot)
      const gate = nextGate
      nextGate = null
      const failed = failNextRead
      failNextRead = false
      if (gate) {
        gate.started = true
        await gate.promise
        pendingGates.delete(gate)
      }
      return send(failed ? 503 : 200, failed ? { error: 'Fixture unavailable' } : snapshot)
    }
    return send(404, { error: 'Unexpected fixture API' })
  }
  response.setHeader(
    'Content-Type',
    request.url === '/bundle.js'
      ? 'text/javascript'
      : request.url === '/style.css'
        ? 'text/css'
        : 'text/html',
  )
  response.end(
    request.url === '/bundle.js'
      ? js
      : request.url === '/style.css'
        ? css
        : '<!doctype html><html><head><link rel="stylesheet" href="/style.css"></head><body style="margin:0"><div id="root"></div><script type="module" src="/bundle.js"></script></body></html>',
  )
})

let browser
try {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const executablePath = process.env.PISPER_SMOKE_BROWSER || chromium.executablePath()
  assert(existsSync(executablePath), 'Install Playwright Chromium or set PISPER_SMOKE_BROWSER')
  browser = await chromium.launch({ headless: true, executablePath })
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, hasTouch: true })
  page.setDefaultTimeout(10000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.goto(`http://127.0.0.1:${server.address().port}`)

  const pill = page.locator('.model-effort-pill')
  const popup = page.locator('.model-effort-popover')
  const slider = page.getByRole('slider', { name: 'Current thinking level', exact: true })
  const picker = page.getByRole('combobox', { name: 'Current chat model', exact: true })
  const waitState = (expectedModel, expectedLevel, levels) =>
    page.waitForFunction(
      ({ expectedModel, expectedLevel, levels }) => {
        const value = window.controls?.inspect()
        return (
          value?.model === expectedModel &&
          value.thinkingModel === expectedModel &&
          value.thinkingLevel === expectedLevel &&
          JSON.stringify(value.availableThinkingLevels) === JSON.stringify(levels) &&
          !value.switchingModel &&
          !value.switchingThinking &&
          !['loading', 'error'].includes(value.thinkingStatus)
        )
      },
      { expectedModel, expectedLevel, levels },
    )
  const open = async () => {
    await pill.click()
    await popup.waitFor()
  }
  const close = async () => {
    await page.keyboard.press('Escape')
    await popup.waitFor({ state: 'hidden' })
  }
  const consistent = async (label) => {
    assert.equal((await pill.textContent()).trim(), `${model}${label}`)
    assert.equal((await page.locator('.model-effort-heading').textContent()).trim(), label)
  }

  await waitState('fixture/reasoner', 'medium', reasonerLevels)
  assert(reads.length > 0, 'SessionPanel refreshes stale capabilities on initial mount')
  await open()
  await waitState('fixture/reasoner', 'medium', reasonerLevels)
  assert.equal(await slider.isEnabled(), true)
  await consistent('Medium')
  await slider.press('End')
  await waitState('fixture/reasoner', 'high', reasonerLevels)
  assert.deepEqual(writes.at(-1), { kind: 'thinking', model: 'reasoner', level: 'high' })
  await consistent('High')

  await picker.selectOption('fixture/fixed')
  await waitState('fixture/fixed', 'off', ['off'])
  assert.equal(await slider.isDisabled(), true)
  await consistent('Off')
  assert.match(await popup.textContent(), /only supports the Off thinking level/)
  const beforeLegacy = reads.length
  legacyModelResponse = true
  await picker.selectOption('fixture/reasoner')
  await waitState('fixture/reasoner', 'medium', reasonerLevels)
  assert(reads.length > beforeLegacy, 'Legacy model response triggers a capability read')
  assert.equal(await slider.isEnabled(), true)
  await consistent('Medium')
  legacyModelResponse = false
  await close()

  // 旧后端读响应甚至没有模型身份；发起时模型/修订号仍必须阻止过期结果回写。
  omitThinkingIdentity = true
  const delayed = holdNextRead()
  const completedBefore = await page.evaluate(() => window.evidence.completedReads)
  await open()
  await page.waitForFunction(() => window.controls.inspect().thinkingStatus === 'loading')
  assert(delayed.started)
  omitThinkingIdentity = false
  await picker.selectOption('fixture/fixed')
  await waitState('fixture/fixed', 'off', ['off'])
  delayed.release()
  await page.waitForFunction((before) => window.evidence.completedReads > before, completedBefore)
  await waitState('fixture/fixed', 'off', ['off'])
  await consistent('Off')
  await picker.selectOption('fixture/reasoner')
  await waitState('fixture/reasoner', 'medium', reasonerLevels)
  await close()

  available.reasoner = ['off', 'high']
  const beforeChanged = reads.length
  await open()
  await waitState('fixture/reasoner', 'high', available.reasoner)
  assert(reads.length > beforeChanged, 'Opening revalidates capabilities of the same model')
  await consistent('High')
  assert.equal(await slider.getAttribute('max'), '1')
  await close()

  failNextRead = true
  await open()
  await page.waitForFunction(() => window.controls.inspect().thinkingStatus === 'error')
  assert.match(await popup.textContent(), /Thinking levels failed to load/)
  assert((await slider.count()) === 0 || (await slider.isDisabled()))
  await close()
  await open()
  await waitState('fixture/reasoner', 'high', available.reasoner)
  assert.equal(await slider.isEnabled(), true)
  await consistent('High')
  await close()

  const shared = holdNextRead()
  const beforeShared = reads.length
  await page.evaluate(() => window.controls.refreshTwice())
  await page.waitForFunction(() => window.controls.inspect().thinkingStatus === 'loading')
  assert(shared.started)
  assert.equal(reads.length, beforeShared + 1, 'Concurrent identical reads are deduplicated')
  shared.release()
  await page.waitForFunction(() => window.evidence.refreshFinished)
  await waitState('fixture/reasoner', 'high', available.reasoner)

  available.reasoner = [...reasonerLevels]
  thinking = 'medium'
  await page.evaluate(() =>
    window.controls.patch({
      thinkingLevel: 'medium',
      thinkingModel: 'fixture/reasoner',
      availableThinkingLevels: ['off'],
      thinkingStatus: 'ok',
    }),
  )
  assert.doesNotMatch(await pill.textContent(), /Medium/, 'Invalid cached effort is not advertised')
  await open()
  await waitState('fixture/reasoner', 'medium', reasonerLevels)
  await consistent('Medium')
  assert.equal(
    writes.some(({ kind }) => kind === 'invalid-thinking'),
    false,
  )
  assert.deepEqual(errors, [])
  console.log(
    'PASS thinking controls: mounted panel refresh, model changes, legacy response, stale reads, same-model refresh, retry, slider, labels and deduplication',
  )
} finally {
  for (const gate of pendingGates) gate.release()
  await browser?.close()
  await new Promise((resolve) => server.close(resolve))
}
