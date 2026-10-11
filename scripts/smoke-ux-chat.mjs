// 聊天错误态回归：使用真实页面、目录 Hook、Dockview 与内存 HTTP 数据。
import assert from 'node:assert/strict'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { createServer } from 'node:http'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const noops = `const noop=()=>{}; const asyncNoop=async()=>{};`
const mocks = {
  '@/app/i18n/use-i18n': `
    const labels={
      'chat:chatPage.initializationFailed':'Chat could not load. Please try again.',
      'chat:chatMessage.retry':'Retry',
      'chat:requestNotice.showDetails':'Show details',
      'chat:requestNotice.incomplete':'Request incomplete',
      'chat:chatDock.newChat':'New chat',
      'chat:chatDock.retryNewChat':'Retry creating chat',
      'chat:chatDock.creatingChat':'Creating…',
      'chat:chatPage.wakingTheAgent':'Loading chat',
    };
    const t=key=>labels[key]||key;
    export function useI18n(){return {t,language:'en-US'}}`,
  '@/lib/ui/shortcuts': 'export function useShortcutLabel(){return "Ctrl+N"}',
  '@/lib/storage/page-state-storage': 'export const pageStateStorage=localStorage',
  '@/stores/client-store':
    'export function useClientStore(select){return select({client:"browser",loaded:true})}',
  '@/stores/runtime-capabilities-store':
    'export function useRuntimeCapabilitiesStore(select){return select({capabilities:null})}',
  '@/components/layout/MobileShellContext':
    'const shell={active:false,mode:"off"}; export function useMobileShell(){return shell}',
  '@/features/chat/hooks/useSessionContextAutoReveal':
    'export function useSessionContextAutoReveal(){}',
  '@/features/chat/hooks/use-live-session-sync': `${noops}
    const state={loadSessionMessages:asyncNoop,syncLiveSession:asyncNoop,loadOlderMessages:asyncNoop};
    export function useLiveSessionSync(){return state}`,
  '@/features/chat/hooks/use-prompt-commands': `${noops}
    const state={sendPrompt:asyncNoop,retryLastTurn:asyncNoop};
    export function usePromptCommands(){return state}`,
  '@/features/chat/hooks/use-session-commands': 'export function useSessionCommands(){return {}}',
  '@/features/chat/components/session/SideChatProvider':
    'export function SideChatProvider({children}){return children}',
  '@/features/chat/components/session/SessionContextLayout':
    'export function SessionContextLayout({children}){return children}',
  '@/features/chat/components/session/SessionContextPanel':
    'export function SessionContextPanel(){return null}',
  '@/features/chat/components/WebPreviewDockPanel':
    'export function WebPreviewDockPanel(){return null}',
  '@/features/chat/components/runtime/FocusSession': `
    import {useEffect} from 'react';
    export function FocusSession({session,error}){
      useEffect(()=>{window.evidence.mounts++},[]);
      return <section data-session-id={session.id}><h2>{session.name}</h2><textarea aria-label="Draft"/>{error&&<div role="alert">{error}</div>}</section>
    }`,
  '@/features/chat/api/chat-api': `
    async function json(path,options){const r=await fetch(path,options);const data=await r.json();if(!r.ok)throw new Error(data.error);return data}
    export const chatApi={
      listSessions:async(options)=>{window.evidence.listOptions.push(options);return json('/api/sessions')},
      getConfig:async(options)=>{window.evidence.configOptions.push(options);return json('/api/config')},
      createSession:async(name,cwd)=>json('/api/sessions',{method:'POST',body:JSON.stringify({name,cwd})}),
    }`,
  '@/lib/http/api': `
    export async function apiJson(path,options){
      if(path==='/api/settings/chat-dock-layout')return null;
      throw new Error('Unexpected fixture API '+path);
    }`,
}
const output = await build({
  entryPoints: [join(root, 'scripts/fixtures/ux-chat.jsx')],
  bundle: true,
  write: false,
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
const css =
  (output.outputFiles.find((file) => file.path.endsWith('.css'))?.text || '') +
  readdirSync(join(root, 'dist/assets'))
    .filter((file) => file.endsWith('.css'))
    .map((file) => readFileSync(join(root, 'dist/assets', file), 'utf8'))
    .join('\n')
const baseline = [{ id: 'alpha', name: 'Alpha', cwd: '/fixture', model: 'fixture/test' }]
let sessions = structuredClone(baseline)
let failSessions = false
let failConfig = false
let failCreate = false
let postGate = null
let listGate = null
let postCount = 0
let listCount = 0
let configCount = 0
const server = createServer(async (request, response) => {
  const json = (status, value) => {
    response.writeHead(status, { 'Content-Type': 'application/json' })
    response.end(JSON.stringify(value))
  }
  if (request.url === '/api/config') {
    configCount++
    return json(
      failConfig ? 503 : 200,
      failConfig
        ? { error: 'Fixture configuration unavailable' }
        : { provider: 'fixture', model: 'test', providers: [] },
    )
  }
  if (request.url === '/api/sessions') {
    if (request.method === 'POST') {
      postCount++
      await postGate?.promise
      if (failCreate) return json(503, { error: 'Fixture creation unavailable' })
      const created = { id: `created-${postCount}`, name: 'Created chat', cwd: '/fixture' }
      sessions = [created, ...sessions]
      return json(201, created)
    }
    listCount++
    await listGate?.promise
    return json(
      failSessions ? 503 : 200,
      failSessions ? { error: 'Fixture sessions unavailable' } : { sessions },
    )
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
        : `<!doctype html><html><head><link rel="stylesheet" href="/style.css"><style>html,body,#root{height:100%;margin:0}#root,#root>div{display:flex;flex-direction:column;min-height:0}#root>div{flex:1}.chat-dock-workspace,.dockview-theme-pisper{height:100%;min-height:0;flex:1}button{font:inherit}</style></head><body><div id="root"></div><script type="module" src="/bundle.js"></script></body></html>`,
  )
})
let browser
try {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const executablePath = process.env.PISPER_SMOKE_BROWSER || chromium.executablePath()
  assert(existsSync(executablePath), 'Install Chromium or set PISPER_SMOKE_BROWSER.')
  browser = await chromium.launch({ headless: true, executablePath })
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
  page.setDefaultTimeout(10000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  const origin = `http://127.0.0.1:${server.address().port}`
  const fresh = async (query = '') => {
    await page.goto('about:blank')
    await page.goto(origin + query)
  }
  for (const failing of ['sessions', 'config']) {
    failSessions = failing === 'sessions'
    failConfig = failing === 'config'
    await fresh()
    await page.getByText('Chat could not load. Please try again.').waitFor()
    assert.equal(await page.locator('.dockview-theme-pisper').count(), 0)
    await page.getByRole('button', { name: 'Show details', exact: true }).click()
    await page
      .getByText(
        failing === 'sessions'
          ? 'Fixture sessions unavailable'
          : 'Fixture configuration unavailable',
        { exact: true },
      )
      .waitFor()
    failSessions = false
    failConfig = false
    const before = { listCount, configCount }
    await page.getByRole('button', { name: 'Retry', exact: true }).click()
    await page.getByRole('heading', { name: 'Alpha', exact: true }).waitFor()
    assert.equal(listCount, before.listCount + 1)
    assert.equal(configCount, before.configCount + 1)
    assert.deepEqual(
      await page.evaluate(() => [
        window.evidence.listOptions.at(-1),
        window.evidence.configOptions.at(-1),
      ]),
      [{ refresh: true }, { refresh: true }],
    )
  }
  assert.equal(postCount, 0, 'Initialization failures must not create sessions')
  // 关闭所有标签后创建失败仍有明确反馈；慢 POST 去重，按钮保持 pending。
  await page
    .getByRole('tab', { name: 'Alpha', exact: true })
    .locator('.dv-default-tab-action')
    .click()
  const create = page.getByRole('button', { name: /^New chat/ })
  await create.waitFor()
  failCreate = true
  postGate = Promise.withResolvers()
  await create.click()
  const pending = page.getByRole('button', { name: /^Creating…/ })
  await pending.waitFor()
  assert.equal(await pending.isDisabled(), true)
  assert.equal(await pending.getAttribute('aria-busy'), 'true')
  await page.waitForFunction(() => document.querySelector('button[aria-busy="true"]') !== null)
  postGate.resolve()
  postGate = null
  await page.getByText('Request incomplete', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'Show details', exact: true }).click()
  await page.getByText('Fixture creation unavailable', { exact: true }).waitFor()
  failCreate = false
  await page.getByRole('button', { name: /^Retry creating chat/ }).click()
  await page.getByRole('heading', { name: 'Created chat', exact: true }).waitFor()
  assert.equal(postCount, 2)
  // 已有目录刷新和初始化重试失败都保留内容；重复重试只产生一组请求。
  sessions = structuredClone(baseline)
  await fresh('/?catalog')
  await page.locator('[data-session-ids="alpha"]').waitFor()
  failSessions = true
  await page.evaluate(() => window.controls.refreshSessions().catch(() => {}))
  assert.equal(await page.locator('[data-session-ids]').getAttribute('data-session-ids'), 'alpha')
  failSessions = false
  failConfig = true
  listGate = Promise.withResolvers()
  const before = { listCount, configCount }
  await page.evaluate(() => {
    window.controls.retryInitialization()
    window.controls.retryInitialization()
  })
  await page.waitForFunction(() => Boolean(window.controls.initializationError))
  assert.equal(await page.locator('[data-session-ids]').getAttribute('data-session-ids'), 'alpha')
  assert.equal(await page.locator('[data-loading]').getAttribute('data-loading'), 'false')
  assert.equal(listCount, before.listCount + 1)
  assert.equal(configCount, before.configCount + 1)
  listGate.resolve()
  listGate = null
  failConfig = false
  await page.evaluate(() => window.controls.retryInitialization())
  await page.waitForFunction(
    () => !window.controls.initializing && !window.controls.initializationError,
  )
  assert.equal(await page.locator('[data-session-ids]').getAttribute('data-session-ids'), 'alpha')
  // 初始化请求未完成时离开，不应等响应返回后悄悄新建会话。
  sessions = []
  listGate = Promise.withResolvers()
  await fresh('/?catalog')
  await page.waitForFunction(() => window.evidence.listOptions.length === 1)
  const listFinished = page.waitForResponse((response) => response.url().endsWith('/api/sessions'))
  await page.evaluate(() => window.setMounted(false))
  listGate.resolve()
  listGate = null
  await listFinished
  await page.evaluate(() => new Promise(requestAnimationFrame))
  assert.equal(postCount, 2)
  assert.deepEqual(errors, [])
  console.log(
    'Chat UX recovery: sessions/config retry, empty Dock create failure/pending/retry, refresh retention and retry deduplication passed.',
  )
} finally {
  postGate?.resolve()
  listGate?.resolve()
  await browser?.close()
  await new Promise((resolve) => server.close(resolve))
}
