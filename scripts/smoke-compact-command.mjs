// Real composer hook and slash menu; controlled HTTP and callbacks never contact a model.
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { join, resolve } from 'node:path'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = resolve(import.meta.dirname, '..')
const fixture = String.raw`
import { useCallback, useRef, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { useFocusComposer } from '@/features/chat/hooks/use-focus-composer'
import { useSessionCommands } from '@/features/chat/hooks/use-session-commands'
import { useLiveSessionSync } from '@/features/chat/hooks/use-live-session-sync'
import { applySessionUpdate, DEFAULT_SESSION_STATE } from '@/lib/session/session-state'
import { ComposerCommandMenu } from '@/features/chat/components/composer/ComposerCommandMenu'
import '@/app/i18n/i18n'
function Fixture() {
  const params = new URLSearchParams(location.search)
  const [value, setValue] = useState('')
  const [attachment, setAttachment] = useState(params.has('attachment'))
  const [sessionId, setSessionId] = useState('fixture')
  const [counts, setCounts] = useState({compact:0,send:0,queue:0,clear:0,pause:0})
  const inputRef = useRef(null)
  const count = key => setCounts(current => ({...current, [key]:current[key]+1}))
  const composer = useFocusComposer({
    sessionId, runMode: 'goal', goalsAvailable:true, teamAvailable:true, workflowsAvailable:true,
    goal:{id:'goal', status:'active', mode:'goal'}, streaming:params.has('streaming'),
    compaction:{active:params.has('compacting')}, hasMessages:!params.has('empty'),
    value, updateValue:setValue,
    selection:{attachments:attachment?[{id:'fixture-file',kind:'file',name:'fixture.txt'}]:[]},
    clearDraft:()=>{count('clear');setValue('');setAttachment(false)}, promptRef:inputRef,
    onCompact:params.has('unsupported')?undefined:async()=>{
      count('compact')
      const response=await fetch('/fixture/compact')
      window.compactResponses=(window.compactResponses||0)+1
      if(params.has('throws'))throw new Error('Fixture compact failure')
      return response.ok
    },
    onGoalPause:()=>count('pause'), onSend:()=>count('send'), onQueue:()=>{count('queue');return true},
    setToolsOpen:()=>{}, requestTranscriptBottom:()=>{},
  })
  return <main style={{padding:100}}>
    <form onSubmit={event=>void composer.submit(event)}>
      <ComposerCommandMenu sessionId={sessionId} value={value} onChange={setValue} inputRef={inputRef}/>
      <textarea aria-label="Draft" ref={inputRef} value={value} onChange={event=>setValue(event.target.value)}/>
      <button type="submit">Submit</button>
      <button type="button" onClick={()=>{
        const event={preventDefault(){}}
        void composer.submit(event);void composer.submit(event)
      }}>Submit twice</button>
    </form>
    <button onClick={()=>composer.setInvocation({kind:'tool',resourceId:'read',resourceName:'Read',behavior:'foreground'})}>Attach resource</button>
    <button onClick={()=>setAttachment(true)}>Attach file</button>
    <button onClick={()=>{setSessionId('other');setValue('Other draft')}}>Switch session</button>
    <output data-testid="counts">{JSON.stringify(counts)}</output>
    <output data-testid="attachments">{attachment?'file':'none'}</output>
    <output data-testid="resource">{composer.invocation?.resourceId||'none'}</output>
    <output data-testid="pending">{String(composer.compactingManually)}</output>
    {composer.commandError&&<p role="alert">{composer.commandError}</p>}
  </main>
}
function SessionCommandFixture() {
  const [value,setValue]=useState('/compact')
  const inputRef=useRef(null)
  const [states,setStates]=useState({fixture:{...DEFAULT_SESSION_STATE,
    messages:[{id:'message',role:'user',text:'Fixture message'}],
    compaction:{active:false,status:'complete',marker:'before'},
  }})
  const statesRef=useRef(states)
  const streamsRef=useRef(new Set())
  const generationsRef=useRef(new Map())
  const updateSessionState=useCallback((id,update)=>{
    const next={...statesRef.current,[id]:applySessionUpdate(statesRef.current[id],update)}
    statesRef.current=next;setStates(next)
  },[])
  const updateSessions=useCallback(()=>[],[])
  const {syncLiveSession}=useLiveSessionSync({sessionStates:states,sessionStatesRef:statesRef,
    localStreamSessionsRef:streamsRef,streamGenerationRef:generationsRef,updateSessionState,updateSessions})
  const commands=useSessionCommands({notify:()=>{},requestText:async()=>null,requestConfirm:async()=>false,
    availableModels:[],sessionStatesRef:statesRef,updateSessionState,updateSessions,
    replaceSessionStates:()=>{},setGlobalError:()=>{},syncLiveSession})
  const composer=useFocusComposer({sessionId:'fixture',goalsAvailable:false,teamAvailable:false,
    workflowsAvailable:false,streaming:states.fixture.streaming,compaction:states.fixture.compaction,
    hasMessages:true,value,updateValue:setValue,selection:{attachments:[]},clearDraft:()=>{throw Error('Must not clear draft')},
    promptRef:inputRef,onCompact:()=>commands.compactSession('fixture'),
    onSend:()=>{throw Error('Must not send')},onQueue:()=>{throw Error('Must not queue')},
    setToolsOpen:()=>{},requestTranscriptBottom:()=>{}})
  return <form onSubmit={event=>void composer.submit(event)}>
    <textarea ref={inputRef} aria-label="Draft" value={value} onChange={event=>setValue(event.target.value)}/>
    <button type="submit">Submit</button>
    <output data-testid="compaction">{JSON.stringify(states.fixture.compaction)}</output>
    <output data-testid="pending">{String(composer.compactingManually)}</output>
    {composer.commandError&&<p role="alert">{composer.commandError}</p>}
  </form>
}
createRoot(document.getElementById('root')).render(new URLSearchParams(location.search).has('real')?<SessionCommandFixture/>:<Fixture/>)
`
const output = await build({
  stdin: { contents: fixture, sourcefile: 'compact-fixture.tsx', resolveDir: root, loader: 'tsx' },
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'browser',
  jsx: 'automatic',
  alias: { '@': join(root, 'src'), '@shared': join(root, 'shared') },
  define: { 'process.env.NODE_ENV': '"production"' },
})
const js = output.outputFiles[0].text
const server = createServer((request, response) => {
  if (request.url === '/fixture.js') {
    response.setHeader('Content-Type', 'text/javascript')
    response.end(js)
    return
  }
  response.setHeader('Content-Type', 'text/html')
  response.end(
    '<!doctype html><html><body><div id="root"></div><script type="module" src="/fixture.js"></script></body></html>',
  )
})
let browser
try {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const base = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_SMOKE_BROWSER
      ? { executablePath: process.env.PISPER_SMOKE_BROWSER }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })
  async function scenario(query = '', handler = (route) => route.fulfill({ json: { ok: true } })) {
    const context = await browser.newContext()
    const page = await context.newPage()
    page.setDefaultTimeout(6000)
    const errors = []
    page.on('pageerror', (error) => errors.push(error.message))
    await page.route('**/api/sessions/*/commands', (route) =>
      route.fulfill({
        json: {
          commands: [
            {
              name: 'compact',
              invocation: '/compact',
              description: 'Remote conflicting prompt',
              argumentHint: '',
              source: 'prompt',
              scope: 'user',
            },
            {
              name: 'review',
              invocation: '/review',
              description: 'Review prompt',
              argumentHint: '',
              source: 'prompt',
              scope: 'user',
            },
          ],
        },
      }),
    )
    await page.route('**/fixture/compact', handler)
    await page.goto(`${base}/?${query}`)
    const input = page.getByLabel('Draft', { exact: true })
    await input.waitFor()
    return {
      context,
      page,
      input,
      errors,
      counts: async () => JSON.parse(await page.getByTestId('counts').textContent()),
    }
  }
  const success = await scenario()
  try {
    await success.input.fill('/comp')
    const option = success.page.getByRole('option').filter({ hasText: '/compact' })
    await option.waitFor()
    assert.equal(
      await option.count(),
      1,
      'The local builtin must override a colliding remote prompt',
    )
    assert.equal((await option.textContent()).includes('Remote conflicting'), false)
    await success.input.press('Enter')
    assert.equal((await success.input.inputValue()).trim(), '/compact')
    await success.page.getByRole('button', { name: 'Submit', exact: true }).click()
    await success.page.waitForFunction(
      () =>
        document.querySelector('[data-testid="pending"]').textContent === 'false' &&
        document.querySelector('textarea').value === '',
    )
    assert.deepEqual(await success.counts(), { compact: 1, send: 0, queue: 0, clear: 0, pause: 0 })
    assert.deepEqual(success.errors, [])
  } finally {
    await success.context.close()
  }
  for (const query of [
    'streaming',
    'compacting',
    'empty',
    'unsupported',
    'attachment',
    'arguments',
    'resource',
  ]) {
    const blocked = await scenario(query)
    try {
      if (query === 'resource')
        await blocked.page.getByRole('button', { name: 'Attach resource' }).click()
      const draft = query === 'arguments' ? '/compact extra' : '/compact'
      await blocked.input.fill(draft)
      await blocked.page.getByRole('button', { name: 'Submit', exact: true }).click()
      await blocked.page.getByRole('alert').waitFor()
      assert.equal(await blocked.input.inputValue(), draft)
      assert.deepEqual(await blocked.counts(), {
        compact: 0,
        send: 0,
        queue: 0,
        clear: 0,
        pause: 0,
      })
      if (query === 'attachment')
        assert.equal(await blocked.page.getByTestId('attachments').textContent(), 'file')
      if (query === 'resource')
        assert.equal(await blocked.page.getByTestId('resource').textContent(), 'read')
      assert.deepEqual(blocked.errors, [])
    } finally {
      await blocked.context.close()
    }
  }
  for (const query of ['failure', 'throws']) {
    const failed = await scenario(query, (route) =>
      route.fulfill({ status: 500, json: { error: 'Fixture failure' } }),
    )
    try {
      await failed.input.fill('/compact')
      await failed.page.getByRole('button', { name: 'Submit', exact: true }).click()
      await failed.page.getByRole('alert').waitFor()
      assert.equal(await failed.input.inputValue(), '/compact')
      assert.deepEqual(await failed.counts(), { compact: 1, send: 0, queue: 0, clear: 0, pause: 0 })
      assert.deepEqual(failed.errors, [])
    } finally {
      await failed.context.close()
    }
  }
  let attempts = 0
  const retry = await scenario('', (route) =>
    route.fulfill({ status: ++attempts === 1 ? 500 : 200, json: {} }),
  )
  try {
    await retry.input.fill('/compact')
    await retry.page.getByRole('button', { name: 'Submit', exact: true }).click()
    await retry.page.getByRole('alert').waitFor()
    await retry.page.getByRole('button', { name: 'Submit', exact: true }).click()
    await retry.page.waitForFunction(() => document.querySelector('textarea').value === '')
    assert.deepEqual(await retry.counts(), { compact: 2, send: 0, queue: 0, clear: 0, pause: 0 })
    assert.equal(await retry.page.getByRole('alert').count(), 0)
  } finally {
    await retry.context.close()
  }
  for (const switchSession of [false, true]) {
    const started = Promise.withResolvers()
    const release = Promise.withResolvers()
    const pending = await scenario('', async (route) => {
      started.resolve()
      await release.promise
      await route.fulfill({ json: { ok: true } })
    })
    try {
      await pending.input.fill('/compact')
      await pending.page.getByRole('button', { name: 'Submit twice' }).click()
      await started.promise
      assert.deepEqual(await pending.counts(), {
        compact: 1,
        send: 0,
        queue: 0,
        clear: 0,
        pause: 0,
      })
      if (switchSession) await pending.page.getByRole('button', { name: 'Switch session' }).click()
      else {
        await pending.input.fill('New draft while compacting')
        await pending.page.getByRole('button', { name: 'Attach file' }).click()
        await pending.page.getByRole('button', { name: 'Attach resource' }).click()
      }
      release.resolve()
      await pending.page.waitForFunction(
        () =>
          window.compactResponses === 1 &&
          document.querySelector('[data-testid="pending"]').textContent === 'false',
      )
      assert.equal(
        await pending.input.inputValue(),
        switchSession ? 'Other draft' : 'New draft while compacting',
      )
      if (!switchSession) {
        assert.equal(await pending.page.getByTestId('attachments').textContent(), 'file')
        assert.equal(await pending.page.getByTestId('resource').textContent(), 'read')
      }
      assert.deepEqual(pending.errors, [])
    } finally {
      release.resolve()
      await pending.context.close()
    }
  }
  for (const serverActive of [false, true]) {
    const context = await browser.newContext()
    const page = await context.newPage()
    page.setDefaultTimeout(6000)
    let posts = 0
    let reads = 0
    const errors = []
    page.on('pageerror', (error) => errors.push(error.message))
    await page.route('**/api/sessions/fixture/compact', (route) => {
      assert.equal(route.request().method(), 'POST')
      posts++
      return route.fulfill(
        posts === 1
          ? { status: 500, json: { error: 'Fixture compact failed' } }
          : {
              json: {
                compaction: { active: false, status: 'complete', marker: 'retry' },
                contextUsage: {},
              },
            },
      )
    })
    await page.route('**/api/sessions/fixture/live', (route) => {
      reads++
      return route.fulfill(
        serverActive
          ? {
              json: {
                streaming: false,
                messages: [{ id: 'message', role: 'user', text: 'Fixture message' }],
                pageInfo: { start: 0 },
                compaction: { active: true, status: 'running', marker: 'server' },
              },
            }
          : { status: 500, json: { error: 'Fixture live failed' } },
      )
    })
    try {
      await page.goto(`${base}/?real`)
      const input = page.getByLabel('Draft', { exact: true })
      await page.getByRole('button', { name: 'Submit', exact: true }).click()
      await page.getByRole('alert').waitFor()
      assert.equal(await input.inputValue(), '/compact')
      assert.equal(reads, 1)
      assert.deepEqual(
        JSON.parse(await page.getByTestId('compaction').textContent()),
        serverActive
          ? { active: true, status: 'running', marker: 'server' }
          : { active: false, status: 'complete', marker: 'before' },
        'Only locally owned optimistic state should roll back after failure',
      )
      await page.getByRole('button', { name: 'Submit', exact: true }).click()
      if (serverActive)
        assert.equal(posts, 1, 'A real server compaction must continue blocking another request')
      else {
        await page.waitForFunction(() => document.querySelector('textarea').value === '')
        assert.equal(posts, 2, 'After POST and live failure a retry must reach the server')
        assert.equal(JSON.parse(await page.getByTestId('compaction').textContent()).marker, 'retry')
      }
      assert.deepEqual(errors, [])
    } finally {
      await context.close()
    }
  }
  console.log(
    'Compact command: builtin menu, success, blocked cases, errors, duplicate submit and draft preservation passed',
  )
} finally {
  await browser?.close()
  await new Promise((resolve, reject) =>
    server.close((error) => (error ? reject(error) : resolve())),
  )
}
