import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'
import { access, lstat, mkdir, readFile, realpath, rm, stat, writeFile } from 'node:fs/promises'
import { basename, dirname, join, resolve } from 'node:path'

// 全部源码、配置、模型调用与文件标记均限于调用方创建的隔离 agent/workspace。
export async function checkPluginsParity({
  check,
  json,
  request,
  chat,
  workspace,
  agent,
  providerId,
  modelId,
  delay,
}) {
  const cwd = await realpath(resolve(workspace))
  const data = await realpath(resolve(agent))
  const before = await json('/api/plugins')
  const appPath = join(data, 'pisper.json')
  const originalApp = JSON.parse(await readFile(appPath, 'utf8'))
  const nonce = randomUUID().replaceAll('-', '')
  const fixture = join(cwd, `plugin-fixture-${nonce}`)
  await mkdir(fixture)
  const ownedPlugins = new Set()
  const ownedSources = new Set()
  const ownedSessions = new Set()
  const streams = new Set()
  const packageFile = join(fixture, 'package.json')
  const packageValue = {
    name: `实际插件-${nonce}`,
    version: '4.5.6',
    scripts: { test: 'synthetic' },
  }
  await writeFile(packageFile, JSON.stringify(packageValue))
  let retained = false

  const prompt = (name, args) =>
    'rust-snapshot-tool:' +
    Buffer.from(JSON.stringify({ name: 'call_tool', args: { name, arguments: args } })).toString(
      'base64',
    )
  const pluginPath = (id) => `/api/plugins/${encodeURIComponent(id)}`
  const capabilityPath = (id, name) => `${pluginPath(id)}/capabilities/${encodeURIComponent(name)}`
  const names = (label) => ({
    id: `fixture-rust-${label}-${nonce}`,
    tool: `fixture_${label}_${nonce}`,
  })
  const local = names('files')
  const created = names('created')
  const commonjs = names('cjs')
  const slowTool = `fixture_wait_${nonce}`
  const heartbeat = join(data, 'plugin-data', local.id, 'heartbeat.txt')
  const receipt = join(data, 'plugin-data', local.id, 'receipt.txt')
  const code = `
import { readFile, writeFile } from 'node:fs/promises';
import { appendFileSync } from 'node:fs';
import { join, basename } from 'node:path';
export async function execute({ toolName, arguments: input, context }) {
  if (toolName === ${JSON.stringify(slowTool)}) {
    const marker = join(context.dataDir, 'heartbeat.txt');
    for (;;) {
      appendFileSync(marker, Buffer.from('actual-worker\\n'));
      await new Promise(resolve => setTimeout(resolve, 20));
    }
  }
  if (input.fail) throw new Error('actual-native-plugin-exception');
  if (input.unsupported) return await import('node:http');
  const file = join(context.cwd, 'package.json');
  const bytes = await readFile(file);
  const pkg = JSON.parse(bytes.toString('utf8'));
  const encoded = Buffer.from(pkg.name, 'utf8').toString('base64');
  const decoded = Buffer.from(encoded, 'base64').toString('utf8');
  await writeFile(join(context.dataDir, 'receipt.txt'), Buffer.from(decoded));
  return { content: [{ type: 'text', text: decoded }], details: {
    name: pkg.name, version: pkg.version, file, basename: basename(file),
    bytes: Buffer.byteLength(bytes), encoded, decoded, cwd: context.cwd,
    sessionId: context.sessionId, dataDir: context.dataDir, environment: process.env
  }};
}
`

  function response(path, method, body) {
    return request(path, {
      method,
      ...(body === undefined
        ? {}
        : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(15000),
    })
  }
  async function expectedError(path, method, body, text) {
    const result = await response(path, method, body)
    const value = await result.json()
    assert.equal(result.status, 500, JSON.stringify(value))
    assert.equal(typeof value.error, 'string')
    assert.match(value.error, text)
    return value
  }
  async function poll(read, predicate, label, timeout = 10000) {
    const deadline = Date.now() + timeout
    let value
    while (Date.now() < deadline) {
      value = await read()
      if (predicate(value)) return value
      await delay(25)
    }
    assert.fail(`${label} did not settle: ${JSON.stringify(value)}`)
  }
  async function absent(path) {
    await assert.rejects(access(path), (error) => error.code === 'ENOENT')
  }
  async function byteSize(path) {
    try {
      return (await stat(path)).size
    } catch (error) {
      if (error.code === 'ENOENT') return 0
      throw error
    }
  }
  async function safeRemove(parent, path, expectedName) {
    let info
    try {
      info = await lstat(path)
    } catch (error) {
      if (error.code === 'ENOENT') return
      throw error
    }
    assert.ok(info.isDirectory() && !info.isSymbolicLink(), 'Owned cleanup refuses links/files')
    const parentPath = await realpath(parent)
    const actual = await realpath(path)
    assert.equal(dirname(actual), parentPath)
    assert.equal(basename(actual), expectedName)
    await rm(actual, { recursive: true })
  }
  async function session(label, sessionCwd = fixture) {
    const value = await json('/api/sessions', 'POST', { name: label, cwd: sessionCwd })
    assert.ok(value.id)
    ownedSessions.add(value.id)
    await json(`/api/sessions/${value.id}/model`, 'PUT', { provider: providerId, model: modelId })
    await json(`/api/sessions/${value.id}/execution-mode`, 'PUT', { mode: 'full-access' })
    return value.id
  }
  async function deleteSession(id) {
    const aborted = await response(`/api/sessions/${id}/abort`, 'POST', {})
    assert.ok([200, 404].includes(aborted.status))
    await poll(
      async () => {
        const result = await response(`/api/sessions/${id}`, 'DELETE')
        if (result.status === 409) return false
        if (result.status === 404) return true
        const value = await result.json()
        assert.equal(result.status, 200, JSON.stringify(value))
        assert.equal(value.deleted, true)
        return true
      },
      Boolean,
      'Delete owned plugin session',
    )
    ownedSessions.delete(id)
  }
  async function source(id, tools, sourceCode, entry = 'index.mjs') {
    const path = join(fixture, `source-${id}`)
    await mkdir(path)
    const manifest = JSON.stringify({
      schemaVersion: 1,
      id,
      name: `Synthetic ${id}`,
      version: '1.0.0',
      entry,
      permissions: ['workspace-read', 'plugin-data-write'],
      unknownManifestField: { preserve: true },
      tools: tools.map((name) => ({
        name,
        description: 'Read or modify only synthetic fixture files.',
        parameters: { type: 'object', properties: { fail: { type: 'boolean' } } },
      })),
    })
    await writeFile(join(path, 'pisper-plugin.json'), manifest)
    await writeFile(join(path, entry), sourceCode)
    return { path, manifest }
  }
  async function install(inspection) {
    const result = await response('/api/plugins/install', 'POST', {
      inspectionId: inspection.inspectionId,
    })
    const value = await result.json()
    assert.equal(result.status, 201, JSON.stringify(value))
    ownedPlugins.add(value.id)
    return value
  }
  async function uninstall(id) {
    const state = await json(pluginPath(id), 'DELETE')
    assert.ok(!state.plugins.some((plugin) => plugin.id === id))
    ownedPlugins.delete(id)
    await absent(join(data, 'plugins', id))
    return state
  }
  async function invoke(id, name, args = {}, failed = false) {
    const events = await chat(prompt(name, args), id)
    const started = events.find(
      (event) => event.event === 'tool_start' && event.data.name === 'call_tool',
    )
    assert.ok(started, JSON.stringify(events))
    const completed = events.find(
      (event) => event.event === 'tool_end' && event.data.id === started.data.id,
    )
    assert.ok(completed, JSON.stringify(events))
    assert.equal(completed.data.error, failed, JSON.stringify(completed))
    if (!failed) assert.equal(completed.data.result?.details?.gatewayToolName, name)
    return completed.data.result
  }
  function openWorker(id) {
    const controller = new AbortController()
    const stream = { id, controller, settled: false }
    stream.pending = request('/api/chat', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        sessionId: id,
        message: prompt(slowTool, {}),
        attachments: [],
        goalMode: false,
        teamMode: false,
      }),
      signal: controller.signal,
    })
      .then(async (result) => {
        assert.equal(result.status, 200)
        assert.match(result.headers.get('content-type') || '', /text\/event-stream/)
        return { raw: await result.text() }
      })
      .then(
        (value) => {
          stream.settled = true
          return value
        },
        (error) => {
          stream.settled = true
          return { error }
        },
      )
    streams.add(stream)
    return stream
  }
  async function finishWorker(stream) {
    await json(`/api/sessions/${stream.id}/abort`, 'POST', {})
    await poll(() => Promise.resolve(stream.settled), Boolean, 'Plugin stream cancellation')
    const result = await stream.pending
    if (result.error) throw result.error
    assert.match(result.raw, /event:\s*(?:done|error)/)
    streams.delete(stream)
    const first = await byteSize(heartbeat)
    await delay(150)
    assert.equal(await byteSize(heartbeat), first, 'Cancelled native worker must stop file writes')
  }
  async function enableCreate() {
    const current = await json('/api/plugins')
    await json('/api/plugins', 'PUT', {
      enabledTools: [...new Set([...current.enabledTools, 'plugin_create'])],
    })
  }
  async function cleanup() {
    const failures = []
    for (const stream of streams) {
      try {
        await finishWorker(stream)
      } catch (error) {
        stream.controller.abort()
        failures.push(error)
      }
    }
    for (const id of [...ownedSessions]) {
      try {
        await deleteSession(id)
      } catch (error) {
        failures.push(error)
      }
    }
    for (const id of [...ownedPlugins]) {
      try {
        await uninstall(id)
      } catch (error) {
        failures.push(error)
      }
    }
    try {
      await json('/api/plugins', 'PUT', {
        enabledTools: before.enabledTools,
        webSearch: before.webSearch,
        piExtensions: before.piExtensions,
        computerUseEnabled: before.computerUseEnabled,
      })
    } catch (error) {
      failures.push(error)
    }
    for (const id of [local.id, created.id, commonjs.id]) {
      try {
        await safeRemove(join(data, 'plugin-data'), join(data, 'plugin-data', id), id)
        if (ownedSources.has(id))
          await safeRemove(join(data, 'plugin-sources'), join(data, 'plugin-sources', id), id)
      } catch (error) {
        failures.push(error)
      }
    }
    if (failures.length === 0) await safeRemove(cwd, fixture, `plugin-fixture-${nonce}`)
    if (failures.length) throw new AggregateError(failures, 'Owned plugin cleanup failed')
  }

  let mainSession
  let originalSource
  let oldInspection
  let restartStream
  try {
    await check(
      'Plugin inspect/install retains exact source bytes and rejects inspection tampering',
      async () => {
        await expectedError('/api/plugins/inspect', 'POST', { path: '' }, /请选择插件目录/)
        originalSource = await source(local.id, [local.tool, slowTool], code)
        const first = await json('/api/plugins/inspect', 'POST', { path: originalSource.path })
        assert.match(first.inspectionId, /^[0-9a-f-]{36}$/)
        assert.equal(first.fileCount, 2)
        assert.match(first.digest, /^[0-9a-f]{64}$/)
        assert.equal(first.plugin.systemAccess, true)
        assert.equal(first.plugin.source, 'local')
        assert.equal(first.plugin.builtIn, false)
        await writeFile(
          join(originalSource.path, 'index.mjs'),
          `${code}\n// changed after inspection`,
        )
        await expectedError(
          '/api/plugins/install',
          'POST',
          { inspectionId: first.inspectionId },
          /发生了变化/,
        )
        await absent(join(data, 'plugins', local.id))
        const fresh = await json('/api/plugins/inspect', 'POST', { path: originalSource.path })
        const installed = await install(fresh)
        assert.equal(installed.id, local.id)
        assert.equal(installed.digest, fresh.digest)
        assert.equal(installed.enabled, true)
        assert.equal(
          await readFile(join(data, 'plugins', local.id, '1.0.0', 'pisper-plugin.json'), 'utf8'),
          originalSource.manifest,
        )
        await expectedError('/api/plugins/inspect', 'POST', { path: originalSource.path }, /已安装/)
        const persistent = JSON.parse(await readFile(join(data, 'pisper-plugins.json'), 'utf8'))
        assert.equal(persistent.version, 1)
        assert.equal(persistent.plugins[local.id].digest, fresh.digest)
        return { id: local.id, sourceBytesPreserved: true, tamperedInspectionRejected: true }
      },
    )

    mainSession = await session('native-plugin-fs-context')
    await check(
      'Actual call_tool runs native fs/path/Buffer with the current Pi session context',
      async () => {
        const result = await invoke(mainSession, local.tool)
        const details = result.details
        assert.equal(details.name, packageValue.name)
        assert.equal(details.version, packageValue.version)
        assert.equal(resolve(details.cwd), fixture)
        assert.equal(resolve(details.file), packageFile)
        assert.equal(details.basename, 'package.json')
        assert.equal(details.sessionId, mainSession)
        assert.equal(resolve(details.dataDir), join(data, 'plugin-data', local.id))
        assert.deepEqual(details.environment, {})
        assert.equal(details.bytes, (await stat(packageFile)).size)
        assert.equal(details.decoded, packageValue.name)
        assert.equal(Buffer.from(details.encoded, 'base64').toString('utf8'), packageValue.name)
        assert.equal(await readFile(receipt, 'utf8'), packageValue.name)
        return { gateway: 'call_tool', name: local.tool, actualFs: true, sessionId: mainSession }
      },
    )

    await check(
      'Plugin/capability enable state and full-access mode guard real execution',
      async () => {
        await expectedError(pluginPath(local.id), 'PATCH', { enabled: 'true' }, /启用状态无效/)
        let state = await json(capabilityPath(local.id, local.tool), 'PATCH', { enabled: false })
        assert.ok(!state.enabledTools.includes(local.tool))
        const oldReceipt = await readFile(receipt)
        const denied = await invoke(mainSession, local.tool, {}, true)
        assert.match(
          JSON.stringify(denied),
          /停用|不可用|unavailable|not available|Unknown tool|not found/,
        )
        assert.deepEqual(await readFile(receipt), oldReceipt)
        state = await json(pluginPath(local.id), 'PATCH', { enabled: false })
        assert.equal(state.plugins.find((plugin) => plugin.id === local.id).enabled, false)
        state = await json(pluginPath(local.id), 'PATCH', { enabled: true })
        assert.ok(state.enabledTools.includes(local.tool) && state.enabledTools.includes(slowTool))
        await json(`/api/sessions/${mainSession}/execution-mode`, 'PUT', {
          mode: 'workspace-write',
        })
        const workspaceState = await json(
          `/api/plugins?sessionId=${encodeURIComponent(mainSession)}`,
        )
        assert.ok(!workspaceState.callableToolNames.includes(local.tool))
        const restricted = await invoke(mainSession, local.tool, {}, true)
        assert.match(
          JSON.stringify(restricted),
          /停用|不可用|unavailable|not available|Unknown tool|not found|完全访问/,
        )
        assert.deepEqual(await readFile(receipt), oldReceipt)
        await json(`/api/sessions/${mainSession}/execution-mode`, 'PUT', { mode: 'full-access' })
        await invoke(mainSession, local.tool)
        const current = await json('/api/plugins')
        const descriptor = current.tools.find((tool) => tool.id === local.tool)
        assert.equal(descriptor.risk, 'high')
        assert.equal(descriptor.effectiveRisk, 'high')
        const app = JSON.parse(await readFile(appPath, 'utf8'))
        for (const [key, value] of Object.entries(originalApp)) {
          if (
            ![
              'toolMode',
              'enabledTools',
              'pluginChanges',
              'pluginsUpdatedAt',
              'webSearch',
              'piExtensions',
              'computerUseEnabled',
            ].includes(key)
          )
            assert.deepEqual(app[key], value, `Plugin config writes must preserve ${key}`)
        }
        return { modeGuard: true, capabilityReload: true, canonicalConfigPreserved: true }
      },
    )

    await check('Real native plugin exceptions propagate through the Pi gateway', async () => {
      const result = await invoke(mainSession, local.tool, { fail: true }, true)
      assert.match(JSON.stringify(result), /actual-native-plugin-exception/)
      const unsupported = await invoke(mainSession, local.tool, { unsupported: true }, true)
      assert.match(JSON.stringify(unsupported), /ERR_PISPER_NODE_COMPAT/)
      return { nativeException: true, gatewayError: true, unsupportedNodeHttpExplicit: true }
    })

    await check(
      'CommonJS entry executes real bundled relative code rather than an echo stub',
      async () => {
        const fixtureSource = await source(
          commonjs.id,
          [commonjs.tool],
          "const {multiply}=require('./math.cjs');module.exports=({arguments:input})=>({content:[{type:'text',text:String(multiply(input.n))}],details:{product:multiply(input.n)}});",
          'index.cjs',
        )
        await writeFile(join(fixtureSource.path, 'math.cjs'), 'exports.multiply=(value)=>value*7;')
        const inspected = await json('/api/plugins/inspect', 'POST', { path: fixtureSource.path })
        await install(inspected)
        const result = await invoke(mainSession, commonjs.tool, { n: 6 })
        assert.equal(result.details.product, 42)
        assert.equal(result.content[0].text, '42')
        await uninstall(commonjs.id)
        return { entry: 'index.cjs', relativeRequire: true, product: 42 }
      },
    )

    await check(
      'Actual plugin_create installs global source and exposes its tool on the next turn',
      async () => {
        await enableCreate()
        const result = await invoke(mainSession, 'plugin_create', {
          id: created.id,
          name: 'Created native plugin fixture',
          permissions: ['workspace-read'],
          tools: [
            {
              name: created.tool,
              description: 'Read a bundled value.',
              parameters: { type: 'object', properties: {} },
            },
          ],
          entryCode:
            "import {value} from './value.mjs';export async function execute({context}){return {content:[{type:'text',text:value}],details:{value,sessionId:context.sessionId}}}",
          files: [{ path: 'value.mjs', content: "export const value='created-bundled-value';" }],
        })
        ownedPlugins.add(created.id)
        ownedSources.add(created.id)
        assert.equal(result.details.id, created.id)
        assert.equal(resolve(result.details.sourcePath), join(data, 'plugin-sources', created.id))
        assert.deepEqual(result.details.tools, [created.tool])
        assert.match(result.content[0].text, /Created and installed plugin/)
        const next = await invoke(mainSession, created.tool)
        assert.equal(next.details.value, 'created-bundled-value')
        assert.equal(next.details.sessionId, mainSession)
        const sourceBytes = await readFile(join(data, 'plugin-sources', created.id, 'index.mjs'))
        const duplicate = await invoke(
          mainSession,
          'plugin_create',
          {
            id: created.id,
            name: 'Must not replace source',
            tools: [
              { name: created.tool, description: 'No overwrite.', parameters: { type: 'object' } },
            ],
            entryCode: 'export const execute=()=>false;',
          },
          true,
        )
        assert.match(JSON.stringify(duplicate), /不能覆盖|已存在|已安装/)
        assert.deepEqual(
          await readFile(join(data, 'plugin-sources', created.id, 'index.mjs')),
          sourceBytes,
        )
        await uninstall(created.id)
        assert.deepEqual(
          await readFile(join(data, 'plugin-sources', created.id, 'index.mjs')),
          sourceBytes,
        )
        return { id: created.id, realCreateTool: true, nextTurnTool: true, sourceRetained: true }
      },
    )

    await check(
      'An active Rust plugin worker rejects uninstall and abort stops its actual file writes',
      async () => {
        const previous = await byteSize(heartbeat)
        const stream = openWorker(mainSession)
        try {
          await poll(
            () => byteSize(heartbeat),
            (size) => size > previous,
            'Actual native worker starts',
          )
          await expectedError(pluginPath(local.id), 'DELETE', undefined, /正在执行.*无法卸载/)
          await finishWorker(stream)
          await uninstall(local.id)
          assert.equal(await readFile(receipt, 'utf8'), packageValue.name)
        } finally {
          if (streams.has(stream)) await finishWorker(stream)
        }
        const inspected = await json('/api/plugins/inspect', 'POST', { path: originalSource.path })
        await install(inspected)
        await invoke(mainSession, local.tool)
        return {
          actualWorker: true,
          activeUninstallRejected: true,
          cancellationReaped: true,
          pluginDataRetained: true,
        }
      },
    )

    await check(
      'Retain a real active plugin worker and installed state for graceful-shutdown/restart verification',
      async () => {
        const inspectionSource = await source(
          `fixture-rust-inspection-${nonce}`,
          [`fixture_inspection_${nonce}`],
          'export const execute=()=>true;',
        )
        oldInspection = await json('/api/plugins/inspect', 'POST', { path: inspectionSource.path })
        const previous = await byteSize(heartbeat)
        restartStream = openWorker(mainSession)
        await poll(
          () => byteSize(heartbeat),
          (size) => size > previous,
          'Actual worker retained for shutdown',
        )
        const state = JSON.parse(await readFile(join(data, 'pisper-plugins.json'), 'utf8'))
        assert.equal(state.plugins[local.id].version, '1.0.0')
        retained = true
        return { id: local.id, activeWorker: true, restartRequired: true }
      },
    )
  } finally {
    if (!retained) await cleanup()
  }

  if (!retained) return undefined
  return async function verifyPluginRestart() {
    try {
      await check(
        'Plugin restart preserves install/enabled/data state, closes old workers and expires inspection tokens',
        async () => {
          await poll(
            () => Promise.resolve(restartStream.settled),
            Boolean,
            'Old worker chat closes at shutdown',
          )
          await restartStream.pending
          streams.delete(restartStream)
          const size = await byteSize(heartbeat)
          await delay(150)
          assert.equal(await byteSize(heartbeat), size, 'Shutdown worker must stop file writes')
          const state = await json('/api/plugins')
          const plugin = state.plugins.find((value) => value.id === local.id)
          assert.ok(plugin && plugin.enabled)
          assert.ok(state.enabledTools.includes(local.tool))
          assert.equal(await readFile(receipt, 'utf8'), packageValue.name)
          await expectedError(
            '/api/plugins/install',
            'POST',
            { inspectionId: oldInspection.inspectionId },
            /已过期/,
          )
          const newSession = await session('native-plugin-after-restart')
          const result = await invoke(newSession, local.tool)
          assert.equal(result.details.sessionId, newSession)
          assert.equal(resolve(result.details.cwd), fixture)
          assert.equal(result.details.name, packageValue.name)
          await uninstall(local.id)
          assert.equal(await readFile(receipt, 'utf8'), packageValue.name)
          return {
            id: local.id,
            nativeWorkerClosed: true,
            persistedPluginExecutes: true,
            oldInspectionExpired: true,
          }
        },
      )
    } finally {
      await cleanup()
    }
  }
}
