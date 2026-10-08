// Browser acceptance owns only loopback pages and its own primary/child chats.
// Root owns the Rust process/model fixture and calls the returned restart proof.
import assert from 'node:assert/strict'
import { createHash, randomUUID } from 'node:crypto'
import { createServer } from 'node:http'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { lstat, mkdir, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { PNG } from 'pngjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const execute = promisify(execFile)
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex')
const marker = (name, args) =>
  'rust-snapshot-tool:' + Buffer.from(JSON.stringify({ name, args })).toString('base64')
const inside = (parent, child) => {
  const value = relative(parent, child)
  return (
    value !== '' &&
    value !== '..' &&
    !value.startsWith('../') &&
    !value.startsWith('..\\') &&
    !isAbsolute(value)
  )
}
const ownAppKeys = new Set(['enabledTools', 'toolMode', 'pluginChanges', 'pluginsUpdatedAt'])
const withoutOwn = (value) =>
  Object.fromEntries(Object.entries(value || {}).filter(([name]) => !ownAppKeys.has(name)))

export async function checkBrowserParity({
  check,
  json,
  request,
  chat,
  workspace,
  agent,
  output,
  providerId,
  modelId,
  fixtureRequests,
  delay,
}) {
  const sandbox = await realpath(resolve(output)),
    cwd = await realpath(resolve(workspace)),
    data = await realpath(resolve(agent))
  assert.ok(inside(sandbox, cwd) && inside(sandbox, data))
  assert.equal(typeof chat, 'function')
  assert.ok(Array.isArray(fixtureRequests))
  const nonce = randomUUID(),
    fixture = join(cwd, `browser-fixture-${nonce}`)
  const sessions = new Set(),
    assets = new Set(),
    pictures = new Set(),
    events = [],
    visits = [],
    ownedRows = new Map(),
    serverErrors = []
  const evidence = {
    nonce,
    actualPiGateway: true,
    ownedLoopbackOnly: true,
    realProfilesTouched: false,
    paidModelsCalled: false,
    ordinaryNodeRuntimeRestored: false,
    steps: [],
    screenshots: [],
    browserProcesses: [],
    remainingOwnedBrowserProcesses: [],
    idleProof:
      'Exact600000ms/injected-clock genuine-browser oracle and native service clock tests; no wall-clock10-minute claim in this smoke',
  }
  let server,
    cross,
    base,
    crossBase,
    initialPlugins,
    initialApp,
    initialDocuments,
    parent,
    second,
    restartDocuments,
    created = false,
    toolsChanged = false,
    cleaned = false,
    oldBackendPid
  const app = async () => JSON.parse(await readFile(join(data, 'pisper.json'), 'utf8'))
  async function documents() {
    return Object.fromEntries(
      await Promise.all(
        ['models.json', 'auth.json', 'settings.json', 'pisper.json'].map(async (name) => [
          name,
          hash(await readFile(join(data, name))),
        ]),
      ),
    )
  }
  const response = (path, method = 'GET', body) =>
    request(path, {
      method,
      ...(body === undefined
        ? {}
        : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(35000),
    })
  async function poll(read, predicate, label, timeout = 15000) {
    const deadline = Date.now() + timeout
    let value
    while (Date.now() < deadline) {
      value = await read()
      if (predicate(value)) return value
      await delay(25)
    }
    assert.fail(`${label}: ${JSON.stringify(value)}`)
  }
  function idleJson(path, method, body, status = 200) {
    return poll(
      async () => {
        const result = await response(path, method, body)
        if (result.status === 409) return null
        const value = await result.json()
        assert.equal(result.status, status, `${method} ${path}: ${JSON.stringify(value)}`)
        return value
      },
      Boolean,
      'Owned browser configuration/session mutation must settle',
    )
  }
  const setTools = (enabledTools) =>
    idleJson('/api/plugins', 'PUT', {
      enabledTools,
      webSearch: initialPlugins.webSearch,
      piExtensions: initialPlugins.piExtensions,
      computerUseEnabled: initialPlugins.computerUseEnabled,
    })
  async function invoke(id, name, args, failed = false) {
    const index = fixtureRequests.length,
      rows = await chat(`${marker(name, args)} owned-browser-${nonce}`, id)
    const start = rows.find((row) => row.event === 'tool_start' && row.data.name === name)
    assert.ok(start, `Actual Pi ${name} must start`)
    const end = rows.find((row) => row.event === 'tool_end' && row.data.id === start.data.id)
    assert.ok(end, `Actual Pi ${name} must finish`)
    assert.equal(end.data.error, failed, JSON.stringify(end.data.result))
    const actual = fixtureRequests.slice(index)
    assert.ok(actual.some((row) => row.tools.includes(name)))
    assert.ok(
      actual.some((row) =>
        row.toolResults.some((result) => result.tool_call_id === start.data.id && result.text),
      ),
    )
    const history = await json(`/api/sessions/${id}/messages?limit=200`)
    assert.ok(
      history.messages.some((message) =>
        message.runActivity?.tools?.some(
          (tool) =>
            tool.id === start.data.id &&
            tool.name === name &&
            typeof tool.output === 'string' &&
            tool.output.length,
        ),
      ),
      'Public history must retain the real completed browser gateway output',
    )
    return { result: end.data.result, events: rows }
  }
  async function browser(id, args) {
    const { result, events } = await invoke(id, 'call_tool', {
      name: 'browser_automation',
      arguments: args,
    })
    assert.equal(result.details.gatewayToolName, 'browser_automation')
    const { gatewayToolName: _name, ...details } = result.details
    assert.equal(details.action, args.action)
    evidence.steps.push({ sessionId: id, input: args, result: details })
    return { details, result, events }
  }
  async function create(name) {
    // release POST /api/sessions returns 201 (sessions-runtime.mjs json(201, ...)).
    const session = await idleJson(
      '/api/sessions',
      'POST',
      {
        name: `browser-${name}-${nonce}`,
        cwd: fixture,
      },
      201,
    )
    sessions.add(session.id)
    await idleJson(`/api/sessions/${session.id}/model`, 'PUT', {
      provider: providerId,
      model: modelId,
    })
    await idleJson(`/api/sessions/${session.id}/execution-mode`, 'PUT', { mode: 'full-access' })
    return session
  }
  async function backendPid() {
    const result = await response('/api/health')
    assert.equal(result.status, 200)
    const url = new URL(result.url)
    assert.equal(url.hostname, '127.0.0.1')
    assert.equal(url.protocol, 'http:')
    assert.ok(url.port)
    const { stdout } = await execute(
      'powershell.exe',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        `$rows=@(Get-NetTCPConnection -LocalAddress 127.0.0.1 -LocalPort ${Number(url.port)} -State Listen -ErrorAction Stop); if($rows.Count -ne 1){throw 'Expected one owned backend listener'}; [int]$rows[0].OwningProcess`,
      ],
      { windowsHide: true },
    )
    const pid = Number(stdout.trim())
    assert.ok(Number.isInteger(pid) && pid > 0 && pid !== 30488)
    return pid
  }
  async function processRows(pid) {
    const script = `$queue=[Collections.Generic.Queue[int]]::new();$queue.Enqueue(${pid});$rows=@();while($queue.Count){$parent=$queue.Dequeue();foreach($p in @(Get-CimInstance Win32_Process -Filter ('ParentProcessId = '+$parent))){$queue.Enqueue([int]$p.ProcessId);if($p.Name -match '^(msedge|chrome|chromium|brave)\\.exe$'){$rows+=@{pid=[int]$p.ProcessId;parent=[int]$p.ParentProcessId;path=$p.ExecutablePath;createdUtc=$p.CreationDate.ToUniversalTime().ToString('o')}}}};ConvertTo-Json -InputObject @($rows) -Compress`
    const { stdout } = await execute(
      'powershell.exe',
      ['-NoProfile', '-NonInteractive', '-Command', script],
      { windowsHide: true },
    )
    return JSON.parse(stdout.trim() || '[]')
  }
  async function rememberProcesses() {
    const rows = await processRows(await backendPid())
    assert.ok(rows.length, 'Browser must be an actual owned Chrome/Edge process')
    for (const row of rows) ownedRows.set(`${row.pid}:${row.createdUtc}`, row)
    evidence.browserProcesses = [...ownedRows.values()]
    return rows
  }
  async function assertGone(rows, label) {
    if (!rows.length) return
    const ids = [...new Set(rows.map((row) => row.pid))]
    await poll(
      async () => {
        const { stdout } = await execute(
          'powershell.exe',
          [
            '-NoProfile',
            '-NonInteractive',
            '-Command',
            `$rows=@();foreach($id in @(${ids.join(',')})){$p=Get-CimInstance Win32_Process -Filter ('ProcessId = '+$id);if($null-ne$p){$rows+=@{pid=[int]$p.ProcessId;createdUtc=$p.CreationDate.ToUniversalTime().ToString('o')}}};ConvertTo-Json -InputObject @($rows) -Compress`,
          ],
          { windowsHide: true },
        )
        return JSON.parse(stdout.trim() || '[]').filter((row) =>
          rows.some((owned) => owned.pid === row.pid && owned.createdUtc === row.createdUtc),
        )
      },
      (rows) => rows.length === 0,
      label,
      10000,
    )
  }
  async function screenshot(session, args, expected) {
    const { details } = await browser(session.id, { action: 'screenshot', ...args })
    const file = await realpath(details.path)
    assert.ok(inside(fixture, file))
    assert.equal(dirname(file), join(fixture, 'generated/browser'))
    assert.equal(details.mimeType, 'image/png')
    pictures.add(file)
    const bytes = await readFile(file),
      png = PNG.sync.read(bytes)
    assert.equal(png.width, expected.width)
    if (expected.height) assert.equal(png.height, expected.height)
    else assert.ok(png.height >= 2200)
    assert.deepEqual(
      [...png.data.subarray((10 * png.width + 10) * 4, (10 * png.width + 10) * 4 + 4)],
      [18, 104, 173, 255],
    )
    const catalog = (await json(`/api/assets?sessionId=${session.id}`)).assets
    const matches = catalog.filter((asset) => asset.filePath && resolve(asset.filePath) === file)
    assert.equal(matches.length, 1, 'Screenshot must be indexed once as a durable asset')
    const asset = matches[0]
    assets.add(asset.id)
    assert.equal(asset.sessionId, session.id)
    assert.equal(asset.source, 'agent')
    assert.equal(asset.mimeType, 'image/png')
    const download = await response(`/api/assets/${asset.id}/download`)
    assert.equal(download.status, 200)
    assert.deepEqual(Buffer.from(await download.arrayBuffer()), bytes)
    const proof = join(output, 'native-browser-screenshots', asset.id + '.png')
    await mkdir(dirname(proof), { recursive: true })
    assert.ok(inside(sandbox, await realpath(dirname(proof))))
    await writeFile(proof, bytes)
    evidence.screenshots.push({
      sessionId: session.id,
      assetId: asset.id,
      path: file,
      retainedProof: proof,
      sha256: hash(bytes),
      width: png.width,
      height: png.height,
      metadataViewport: details.viewport,
    })
    return details
  }
  async function safeRemove() {
    if (!created) return
    const info = await lstat(fixture)
    assert.ok(info.isDirectory() && !info.isSymbolicLink())
    const real = await realpath(fixture)
    assert.equal(dirname(real), cwd)
    assert.equal(basename(real), `browser-fixture-${nonce}`)
    await rm(real, { recursive: true })
    created = false
  }
  async function cleanup() {
    if (cleaned) return
    const failures = []
    for (const id of sessions) {
      try {
        await response(`/api/sessions/${id}/abort`, 'POST', {})
        await poll(
          async () => {
            const result = await response(`/api/sessions/${id}`, 'DELETE')
            if (result.status === 409) return false
            assert.ok([200, 404].includes(result.status))
            return true
          },
          Boolean,
          'Delete owned browser session and close its browser',
        )
      } catch (error) {
        failures.push(error)
      }
    }
    sessions.clear()
    for (const id of assets) {
      try {
        const result = await response(`/api/assets/${id}`, 'DELETE')
        assert.ok([200, 404].includes(result.status))
      } catch (error) {
        failures.push(error)
      }
    }
    if (toolsChanged) {
      try {
        await setTools([
          ...new Set([...(initialApp.enabledTools || []), ...initialPlugins.enabledTools]),
        ])
        assert.deepEqual((await json('/api/plugins')).enabledTools, initialPlugins.enabledTools)
        assert.deepEqual(withoutOwn(await app()), withoutOwn(initialApp))
        const current = await documents()
        for (const name of ['models.json', 'auth.json', 'settings.json'])
          assert.equal(current[name], initialDocuments[name], `Browser helper changed ${name}`)
      } catch (error) {
        failures.push(error)
      }
    }
    try {
      await assertGone([...ownedRows.values()], 'Every owned browser process must be closed')
    } catch (error) {
      failures.push(error)
    }
    if (server) {
      server.closeAllConnections()
      cross.closeAllConnections()
      await Promise.all([
        new Promise((done) => server.close(done)),
        new Promise((done) => cross.close(done)),
      ])
      server = undefined
    }
    if (!failures.length) await safeRemove()
    if (failures.length) throw new AggregateError(failures, 'Owned browser cleanup failed')
    cleaned = true
    evidence.remainingOwnedBrowserProcesses = []
    await writeFile(
      join(output, 'native-browser-proof.json'),
      JSON.stringify(evidence, null, 2) + '\n',
    )
  }
  const restart = async () => {
    assert.ok(restartDocuments, 'prepareRestart must freeze documents immediately before shutdown')
    try {
      await check(
        'Native browser restart closes old processes and keeps durable screenshot assets',
        async () => {
          await assertGone([...ownedRows.values()], 'Restart must reap every old owned browser')
          assert.deepEqual(await documents(), restartDocuments)
          const inspected = await browser(parent.id, { action: 'inspect' })
          assert.equal(inspected.details.url, 'about:blank')
          assert.equal(inspected.details.title, '')
          await rememberProcesses()
          await browser(parent.id, { action: 'close' })
          for (const shot of evidence.screenshots) {
            const bytes = await readFile(shot.path)
            assert.equal(hash(bytes), shot.sha256)
            const asset = await response(`/api/assets/${shot.assetId}/download`)
            assert.equal(asset.status, 200)
            assert.deepEqual(Buffer.from(await asset.arrayBuffer()), bytes)
          }
          return {
            durableScreenshots: evidence.screenshots.length,
            oldBackendPid,
            newBackendPid: await backendPid(),
            oldBrowserProcessesReaped: true,
          }
        },
      )
    } finally {
      await cleanup()
    }
  }
  restart.prepareRestart = async () => {
    restartDocuments = await documents()
    oldBackendPid = await backendPid()
    evidence.restartBaselineDocuments = restartDocuments
    return { documents: restartDocuments, oldBackendPid }
  }
  restart.dispose = cleanup
  try {
    const validated = await check(
      'Native browser real Pi discovery, trusted selectors, PNG assets and primary-session isolation',
      async () => {
        initialPlugins = await json('/api/plugins')
        initialApp = await app()
        initialDocuments = await documents()
        await mkdir(fixture)
        created = true
        const html = await readFile(
          join(root, 'runtime-rs/src/native_browser/oracles/fixture.html'),
          'utf8',
        )
        const handler = (req, res) => {
          try {
            if (req.url === '/events') {
              const chunks = []
              req.on('data', (chunk) => chunks.push(chunk))
              req.on('end', () => {
                events.push(JSON.parse(Buffer.concat(chunks).toString()))
                res.writeHead(204)
                res.end()
              })
              return
            }
            visits.push({ url: req.url })
            res.setHeader('Content-Type', 'text/html; charset=utf-8')
            if (req.url?.startsWith('/submitted'))
              return res.end(
                '<title>Owned Submitted</title><body>Owned submitted ' +
                  new URL(req.url, base).searchParams.get('query') +
                  '</body>',
              )
            if (req.url === '/frame')
              return res.end(
                "<button id=\"frame-button\" onclick=\"fetch('/events',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({type:'click',trusted:event.isTrusted,target:'frame-button'})});this.textContent=event.isTrusted?'Frame trusted':'Frame untrusted'\">Frame button</button><input id=\"frame-input\" oninput=\"fetch('/events',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({type:'input',trusted:event.isTrusted,target:'frame-input',value:this.value})})\">",
              )
            res.end(
              html.replace(
                '<iframe id="cross-frame"></iframe>',
                `<iframe id="cross-frame" src="${crossBase}/frame"></iframe>`,
              ),
            )
          } catch (error) {
            serverErrors.push(error)
            res.writeHead(500)
            res.end()
          }
        }
        server = createServer(handler)
        cross = createServer(handler)
        await new Promise((done) => cross.listen(0, 'localhost', done))
        crossBase = `http://localhost:${cross.address().port}`
        await new Promise((done) => server.listen(0, '127.0.0.1', done))
        base = `http://127.0.0.1:${server.address().port}`
        toolsChanged = true
        await setTools([
          ...new Set([
            ...(initialApp.enabledTools || []),
            ...initialPlugins.enabledTools,
            'browser_automation',
          ]),
        ])
        parent = await create('primary')
        second = await create('second')
        const discovered = (
          await invoke(parent.id, 'discover_tools', {
            query: 'browser_automation controlled browser',
            limit: 5,
          })
        ).result
        assert.ok(discovered.details.matches.some((match) => match.name === 'browser_automation'))
        const opened = await browser(parent.id, {
          action: 'open',
          url: base,
          width: 1440,
          height: 900,
        })
        assert.equal(opened.details.title, 'Owned Browser Fixture')
        const firstProcesses = await rememberProcesses()
        const inspected = await browser(parent.id, { action: 'inspect' })
        assert.equal(inspected.details.url, base + '/')
        assert.ok(inspected.details.text.includes('Owned browser contract'))
        assert.ok(
          inspected.details.elements.some(
            (row) => row.selector === '#trust' && row.text === 'Trusted click',
          ),
        )
        assert.ok(
          inspected.details.elements.some((row) => row.selector === 'input[name="named field"]'),
        )
        assert.ok(inspected.details.elements.some((row) => row.selector === '[data-testid="hint"]'))
        assert.ok(
          !inspected.details.elements.some(
            (row) => row.text === 'Hidden' || row.text === 'Invisible',
          ),
        )
        const oracle = JSON.parse(
          await readFile(
            join(root, 'runtime-rs/src/native_browser/oracles/node24-browser-contract.json'),
            'utf8',
          ),
        )
        const reference = oracle.steps.find((step) => step.label === 'inspect-hints').value
        const normalize = (value, origin) =>
          JSON.parse(
            JSON.stringify(value, (_key, value) =>
              typeof value === 'string' ? value.replaceAll(origin, '<owned-fixture>') : value,
            ),
          )
        assert.deepEqual(
          normalize(inspected.details, base),
          normalize(reference, oracle.fixtureBase),
          'Native inspect must match actual Node/Edge reference on the identical page',
        )
        evidence.referenceInspectMatched = true
        for (const selector of [
          '#trust',
          'role=button[name="Trusted click"]',
          'text=First duplicate',
          'xpath=//button[@id="trust"]',
          '.duplicate',
          'css=button.duplicate >> nth=1',
        ])
          await browser(parent.id, { action: 'click', selector })
        await poll(
          () => events,
          (rows) => rows.some((row) => row.type === 'click' && row.trusted),
          'Native CDP click must create trusted DOM input',
        )
        await browser(parent.id, {
          action: 'type',
          selector: '#value',
          text: 'owned browser 😀',
          submit: false,
        })
        await poll(
          () => events,
          (rows) =>
            rows.some(
              (row) => row.type === 'input' && row.trusted && row.value === 'owned browser 😀',
            ),
          'Native fill must replace input and emit trusted input',
        )
        await screenshot(parent, { outputName: '../unsafe name.png' }, { width: 1440 })
        const viewport = await screenshot(
          parent,
          { outputName: 'viewport.png', width: 700, height: 500, fullPage: false },
          { width: 1440, height: 900 },
        )
        assert.deepEqual(viewport.viewport, { width: 700, height: 500 })
        await browser(parent.id, {
          action: 'type',
          selector: '#value',
          text: 'submitted browser 😀',
          submit: true,
        })
        await poll(
          () => events,
          (rows) =>
            rows.some((row) => row.type === 'keydown' && row.key === 'Enter' && row.trusted),
          'Native Enter must produce trusted keyboard input',
        )
        const submitted = await browser(parent.id, { action: 'inspect' })
        assert.ok(submitted.details.url.startsWith(base + '/submitted?'))
        assert.equal(submitted.details.title, 'Owned Submitted')
        await browser(parent.id, { action: 'open', url: base })
        for (const frame of ['#same-frame', '#cross-frame'])
          await browser(parent.id, {
            action: 'click',
            selector: frame + ' >> internal:control=enter-frame >> #frame-button',
          })
        await browser(parent.id, {
          action: 'type',
          selector: '#cross-frame >> internal:control=enter-frame >> #frame-input',
          text: 'owned cross-frame',
        })
        await poll(
          () => events,
          (rows) =>
            rows.filter(
              (row) => row.type === 'click' && row.trusted && row.target === 'frame-button',
            ).length >= 2 &&
            rows.some(
              (row) =>
                row.type === 'input' &&
                row.trusted &&
                row.target === 'frame-input' &&
                row.value === 'owned cross-frame',
            ),
          'Native same/cross-origin iframe selectors must deliver trusted click/fill',
        )
        evidence.framesTrusted = true
        await browser(second.id, { action: 'inspect' })
        const rowsWithSecond = await rememberProcesses()
        const secondProcesses = rowsWithSecond.filter(
          (row) =>
            !firstProcesses.some(
              (first) => first.pid === row.pid && first.createdUtc === row.createdUtc,
            ),
        )
        assert.ok(secondProcesses.length)
        const secondBlank = await browser(second.id, { action: 'inspect' })
        assert.equal(secondBlank.details.url, 'about:blank')
        await browser(second.id, { action: 'open', url: base })
        const secondState = await browser(second.id, { action: 'inspect' })
        assert.ok(secondState.details.text.includes('clicks:0'))
        const childMarker = marker('call_tool', {
          name: 'browser_automation',
          arguments: { action: 'open', url: base },
        })
        const beforeVisits = visits.length,
          index = fixtureRequests.length
        const child = await idleJson(`/api/sessions/${parent.id}/agents`, 'POST', {
          taskName: 'browser-denied-' + nonce.slice(0, 8),
          message: childMarker,
          role: 'Browser negative test',
        })
        const childId = child.agent?.id || child.id
        assert.ok(childId)
        const terminal = await poll(
          () => json(`/api/sessions/${parent.id}/agents`),
          (value) =>
            value.agents?.some(
              (row) =>
                row.id === childId && ['completed', 'failed', 'interrupted'].includes(row.status),
            ),
          'Primary-only browser denial must settle through a real child Agent',
          20000,
        )
        const childEntry = terminal.agents.find((row) => row.id === childId)
        const childResults = fixtureRequests.slice(index).flatMap((row) => row.toolResults)
        assert.ok(
          childResults.some(
            (result) =>
              /browser_automation/.test(result.text) &&
              /disabled|unavailable|primary|主|不可用|停用/i.test(result.text),
          ),
          'Real child tool result must reject browser access',
        )
        assert.equal(
          visits.length,
          beforeVisits,
          'Denied child must not navigate an actual browser',
        )
        evidence.primaryOnly = {
          childId,
          terminalStatus: childEntry.status,
          realToolRejection: true,
          noNavigation: true,
        }
        await browser(second.id, { action: 'close' })
        await assertGone(secondProcesses, 'Explicit close must reap the second session browser')
        const beforeReopen = await processRows(await backendPid())
        await browser(second.id, { action: 'open', url: base })
        const reopened = await rememberProcesses()
        const deletedProcesses = reopened.filter(
          (row) =>
            !beforeReopen.some((old) => old.pid === row.pid && old.createdUtc === row.createdUtc),
        )
        assert.ok(deletedProcesses.length)
        const deleted = await idleJson(`/api/sessions/${second.id}`, 'DELETE')
        assert.equal(deleted.deleted, true)
        sessions.delete(second.id)
        await assertGone(
          deletedProcesses,
          'Deleting an active browser-owning session must reap its browser',
        )
        evidence.activeDeletionProcessesReaped = true
        await browser(parent.id, { action: 'close' })
        await assertGone(firstProcesses, 'Primary browser close must reap its actual processes')
        await browser(parent.id, { action: 'open', url: base })
        await rememberProcesses()
        assert.deepEqual(serverErrors, [])
        const after = await documents()
        for (const name of ['models.json', 'auth.json', 'settings.json'])
          assert.equal(after[name], initialDocuments[name])
        assert.deepEqual(withoutOwn(await app()), withoutOwn(initialApp))
        return {
          actualPiGateway: true,
          selectors: 'CSS/text/role/xpath/nth first-match and same/cross-origin frames',
          trustedClick: true,
          trustedFill: true,
          trustedEnter: true,
          framesTrusted: true,
          fullPagePngPixels: true,
          viewportMetadataMatchesRelease: true,
          durableScreenshotAssets: evidence.screenshots.length,
          isolatedPrimarySessions: true,
          primaryOnlyChildDenied: true,
          explicitCloseProcessesReaped: true,
          activeDeletionProcessesReaped: true,
          remainingBrowserReservedForRestart: true,
        }
      },
    )
    if (!validated) {
      await cleanup()
      return undefined
    }
    return restart
  } catch (error) {
    await cleanup().catch((cleanupError) => {
      throw new AggregateError([error, cleanupError], 'Browser acceptance and cleanup failed')
    })
    throw error
  }
}
