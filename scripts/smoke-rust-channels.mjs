import assert from 'node:assert/strict'
import { createHash, randomUUID } from 'node:crypto'
import { lstat, mkdir, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path'
import { PNG } from 'pngjs'

const sha = (bytes) => createHash('sha256').update(bytes).digest('hex')
function inside(root, path) {
  const child = relative(root, path)
  return (
    child !== '' &&
    child !== '..' &&
    !child.startsWith('../') &&
    !child.startsWith('..\\') &&
    !isAbsolute(child)
  )
}
function sourcePng() {
  const image = new PNG({ width: 8, height: 4 })
  for (let index = 0; index < image.data.length; index += 4)
    image.data.set([43, 83, 123, 255], index)
  return PNG.sync.write(image)
}
const marker = (name, args, label) =>
  'rust-snapshot-tool:' +
  Buffer.from(JSON.stringify({ name, args })).toString('base64') +
  ' ' +
  label

// 所有协议请求指向本 helper 的 Telegram HTTP 夹具；真实 Agent、审批和模板路由由调用方拥有。
export async function checkChannelsParity({
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
  const sandbox = await realpath(resolve(output))
  const cwd = await realpath(resolve(workspace))
  const data = await realpath(resolve(agent))
  assert.ok(inside(sandbox, cwd) && inside(sandbox, data))
  assert.equal(typeof chat, 'function')
  assert.ok(Array.isArray(fixtureRequests))
  const nonce = randomUUID()
  const fixture = join(cwd, `channel-fixture-${nonce}`)
  const token = `987654321:${nonce.replaceAll('-', '')}`
  const owner = 77770001
  const firstPeer = 700000000 + Math.floor(Math.random() * 100000000)
  const secondPeer = firstPeer + 1
  const deniedPeer = firstPeer + 2
  const png = sourcePng()
  const sourceName = `channel-reference-${nonce}`
  const proof = `Native Telegram channel tool and asset proof ${nonce}\n`
  const firstOutput = join(fixture, 'generated', 'channel-proof.txt')
  const peerProof = join(fixture, 'peer-proof.txt')
  const changedDirectory = join(fixture, 'changed-directory')
  const event = 'chat.completed'
  const templateRoute = `/api/settings/notifications/templates/${event}/telegram`
  const ownedSessions = new Set()
  const ownedAssets = new Set()
  const initialAssetIds = new Set()
  const calls = []
  const pending = new Set()
  let updates = []
  let updateId = 1000
  let messageId = 2000
  let server
  let apiBase
  let baseline
  let initialDocuments
  let restartDocuments
  let initialTemplate
  let initialChannels
  let connected = false
  let templateChanged = false
  let createdFixture = false
  let retained
  let cleaned = false
  const errors = []

  const channelFile = () => readFile(join(data, 'pisper-channels.json'), 'utf8').then(JSON.parse)
  async function documents() {
    return Object.fromEntries(
      await Promise.all(
        ['models.json', 'auth.json', 'settings.json', 'pisper.json'].map(async (name) => [
          name,
          sha(await readFile(join(data, name))),
        ]),
      ),
    )
  }
  function response(path, method = 'GET', body) {
    return request(path, {
      method,
      ...(body === undefined
        ? {}
        : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(20000),
    })
  }
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
  function send(res, result) {
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.end(JSON.stringify({ ok: true, result }))
  }
  function deliver(entry) {
    const available = updates.filter((update) => update.update_id >= entry.offset)
    if (available.length && !entry.res.destroyed) {
      pending.delete(entry)
      send(entry.res, available)
    }
  }
  function inbound(content, { peer = firstPeer, sender = owner, photo = false } = {}) {
    const id = ++messageId
    updates.push({
      update_id: ++updateId,
      message: {
        message_id: id,
        date: 1700000000,
        chat: { id: peer, type: 'private' },
        from: { id: sender, first_name: `Synthetic ${nonce}` },
        ...(photo ? { caption: content, photo: [{ file_id: sourceName }] } : { text: content }),
      },
    })
    for (const entry of [...pending]) deliver(entry)
    return id
  }
  const replies = (id) =>
    calls.filter(
      (call) => call.method === 'sendMessage' && call.body.reply_parameters?.message_id === id,
    )
  function reply(id, predicate = () => true, label = 'Actual Telegram reply') {
    return poll(
      () => replies(id),
      (rows) => rows.some((row) => predicate(row.body.text)),
      label,
    )
  }
  async function scope(peer = firstPeer) {
    return (await json('/api/channels')).scopes.find((entry) => entry.key === `telegram:${peer}`)
  }
  async function recordSession(peer = firstPeer) {
    const value = await poll(
      () => scope(peer),
      (entry) => Boolean(entry?.sessionId),
      'Native channel must own a real durable session',
    )
    ownedSessions.add(value.sessionId)
    return value
  }
  async function assertTool(id, name, outputText) {
    const history = await json(`/api/sessions/${id}/messages?limit=200`)
    const tools = history.messages.flatMap((message) => message.runActivity?.tools || [])
    assert.ok(
      tools.some(
        (tool) => tool.name === name && tool.status === 'done' && tool.output?.includes(outputText),
      ),
      'Public restored history must contain the actual completed tool output',
    )
    assert.ok(
      fixtureRequests.some((entry) =>
        entry.toolResults.some((result) => result.text.includes(outputText)),
      ),
      'The actual local model must receive the real channel tool result',
    )
  }
  async function joinSession(id) {
    const abort = await response(`/api/sessions/${id}/abort`, 'POST', {})
    assert.ok([200, 404].includes(abort.status))
    await poll(
      async () => {
        const result = await response(`/api/sessions/${id}`, 'DELETE')
        if (result.status === 409) return false
        assert.ok([200, 404].includes(result.status))
        return true
      },
      Boolean,
      'Owned channel Agent task must join before files are removed',
    )
  }
  async function cleanup() {
    if (cleaned) return
    const failures = []
    const attempt = async (action) => {
      try {
        await action()
      } catch (error) {
        failures.push(error)
      }
    }
    if (connected)
      await attempt(async () => {
        const current = await json('/api/channels')
        for (const item of current.scopes.filter(
          (item) =>
            item.platform === 'telegram' &&
            [String(firstPeer), String(secondPeer)].includes(String(item.peerId)),
        ))
          if (item.sessionId) ownedSessions.add(item.sessionId)
        await json('/api/channels/telegram', 'PATCH', { enabled: false })
        const listed = await json('/api/sessions')
        const sessions = Array.isArray(listed) ? listed : listed.sessions
        assert.ok(Array.isArray(sessions))
        for (const session of sessions) {
          if (
            session.cwd &&
            (resolve(session.cwd) === fixture || inside(fixture, resolve(session.cwd)))
          ) {
            ownedSessions.add(session.id)
          }
        }
        for (const id of ownedSessions) {
          await joinSession(id)
          for (const asset of (await json(`/api/assets?sessionId=${encodeURIComponent(id)}`))
            .assets) {
            if (!initialAssetIds.has(asset.id)) ownedAssets.add(asset.id)
          }
        }
        await json('/api/channels/telegram', 'DELETE')
        connected = false
      })
    if (templateChanged)
      await attempt(async () => {
        await json(templateRoute, 'PUT', {
          enabled: initialTemplate.enabled,
          content: initialTemplate.channels.telegram.content,
        })
        templateChanged = false
      })
    for (const id of ownedAssets)
      await attempt(async () => {
        const result = await response(`/api/assets/${encodeURIComponent(id)}`, 'DELETE')
        assert.ok([200, 404].includes(result.status))
      })
    if (initialDocuments)
      await attempt(async () =>
        assert.deepEqual(await documents(), restartDocuments || initialDocuments),
      )
    if (baseline && !connected)
      await attempt(async () =>
        assert.deepEqual(
          await channelFile(),
          baseline,
          'Unknown private channel/scopes/template fields must remain intact after owned cleanup',
        ),
      )
    if (server)
      await attempt(async () => {
        server.closeAllConnections()
        await new Promise((done, reject) =>
          server.close((error) => (error ? reject(error) : done())),
        )
        assert.equal(server.listening, false)
        server = undefined
      })
    if (createdFixture && failures.length === 0)
      await attempt(async () => {
        const info = await lstat(fixture)
        assert.ok(info.isDirectory() && !info.isSymbolicLink())
        const actual = await realpath(fixture)
        assert.equal(dirname(actual), cwd)
        assert.equal(basename(actual), `channel-fixture-${nonce}`)
        await rm(actual, { recursive: true })
        createdFixture = false
      })
    if (failures.length)
      throw new AggregateError(
        failures,
        failures.map((error) => error.stack || String(error)).join('\n'),
      )
    cleaned = true
  }

  await check(
    'Native Telegram inbound updates execute real Pi tools, bypass pending approvals, deliver assets and notifications, and persist isolated channel scopes',
    async () => {
      try {
        initialChannels = await json('/api/channels')
        assert.deepEqual(
          Object.keys(initialChannels).sort(),
          ['providers', 'connections', 'scopes', 'models'].sort(),
        )
        assert.deepEqual(
          initialChannels.providers.map((provider) => provider.type).sort(),
          ['feishu', 'weixin', 'qq', 'telegram'].sort(),
        )
        assert.equal(
          initialChannels.connections.telegram,
          null,
          'The synthetic baseline must not own an older Telegram connection',
        )
        assert.equal(
          initialChannels.scopes.some((entry) => entry.platform === 'telegram'),
          false,
        )
        baseline = await channelFile()
        initialDocuments = await documents()
        for (const asset of (await json('/api/assets')).assets) initialAssetIds.add(asset.id)
        const config = JSON.parse(await readFile(join(data, 'models.json'), 'utf8')).providers[
          providerId
        ]
        const model = config.models.find((entry) => entry.id === modelId)
        assert.ok(model)
        assert.ok(
          ['127.0.0.1', 'localhost', '[::1]'].includes(
            new URL(model.baseUrl || config.baseUrl).hostname,
          ),
          'Only the owned synthetic local model may receive channel prompts',
        )
        const settings = await json('/api/settings/notifications')
        initialTemplate = settings.templates.find((template) => template.id === event)
        assert.ok(initialTemplate)
        await mkdir(fixture)
        createdFixture = true
        await mkdir(changedDirectory)
        await writeFile(peerProof, `Native other peer proof ${nonce}\n`)
        server = createServer(async (req, res) => {
          try {
            const url = new URL(req.url, 'http://127.0.0.1')
            if (req.method === 'GET' && url.pathname === `/file/bot${token}/reference.png`) {
              calls.push({ method: 'downloadReference', size: png.length, sha256: sha(png) })
              res.writeHead(200, { 'Content-Type': 'image/png', 'Content-Length': png.length })
              res.end(png)
              return
            }
            assert.equal(req.method, 'POST')
            assert.ok(
              url.pathname.startsWith(`/bot${token}/`),
              'The transport may only use its own synthetic Telegram credential',
            )
            const method = url.pathname.slice(`/bot${token}/`.length)
            const chunks = []
            for await (const chunk of req) chunks.push(chunk)
            const bytes = Buffer.concat(chunks)
            assert.ok(
              bytes.length < 1024 * 1024,
              'Owned outgoing Telegram request must stay bounded',
            )
            if (method === 'sendDocument' || method === 'sendPhoto') {
              assert.match(req.headers['content-type'], /^multipart\/form-data;/)
              const form = await new Response(bytes, {
                headers: { 'Content-Type': req.headers['content-type'] },
              }).formData()
              const file = form.get(method === 'sendPhoto' ? 'photo' : 'document')
              assert.ok(file && typeof file !== 'string')
              const actual = Buffer.from(await file.arrayBuffer())
              calls.push({
                method,
                peer: form.get('chat_id'),
                name: file.name,
                mimeType: file.type,
                bytes: actual,
                size: actual.length,
                sha256: sha(actual),
              })
              send(res, { message_id: ++messageId })
              return
            }
            assert.match(req.headers['content-type'], /application\/json/)
            const body = JSON.parse(bytes)
            calls.push({ method, body })
            if (method === 'getMe')
              send(res, {
                id: 987654321,
                first_name: 'Native Acceptance Bot',
                username: 'native_acceptance_bot',
              })
            else if (method === 'getUpdates') {
              assert.equal(body.timeout, 30)
              assert.deepEqual(body.allowed_updates, ['message'])
              assert.ok(Number.isInteger(body.offset))
              updates = updates.filter((update) => update.update_id >= body.offset)
              const entry = { offset: body.offset, res }
              pending.add(entry)
              res.on('close', () => pending.delete(entry))
              deliver(entry)
            } else if (method === 'getFile') {
              assert.equal(body.file_id, sourceName)
              send(res, { file_path: 'reference.png', file_size: png.length })
            } else if (method === 'sendMessage') {
              assert.equal(typeof body.chat_id, 'string')
              assert.equal(typeof body.text, 'string')
              assert.ok(body.text.length <= 4096)
              send(res, { message_id: ++messageId })
            } else assert.fail(`Unexpected owned Telegram method ${method}`)
          } catch (error) {
            errors.push(error)
            if (!res.headersSent) res.writeHead(500, { 'Content-Type': 'application/json' })
            res.end(
              JSON.stringify({ ok: false, description: 'Owned Telegram fixture rejected request' }),
            )
          }
        })
        await new Promise((done, reject) => {
          server.once('error', reject)
          server.listen(0, '127.0.0.1', done)
        })
        server.unref()
        server.on('connection', (socket) => socket.unref())
        apiBase = `http://127.0.0.1:${server.address().port}`
        assert.equal(new URL(apiBase).hostname, '127.0.0.1')
        connected = true
        const connectedState = await json('/api/channels/telegram/onboarding', 'POST', {
          token,
          accountId: '987654321',
          ownerUserId: String(owner),
          baseUrl: apiBase,
          fileBaseUrl: apiBase + '/file',
        })
        assert.equal(connectedState.connections.telegram.status, 'connected')
        assert.equal(connectedState.connections.telegram.ownerConfigured, true)
        assert.equal(JSON.stringify(connectedState).includes(token), false)
        await json('/api/channels/telegram', 'PATCH', {
          accessMode: 'owner',
          defaultCwd: fixture,
          executionMode: 'approval-required',
          runMode: 'plan',
          replyModel: { provider: providerId, model: modelId },
        })

        const modelStart = fixtureRequests.length
        const firstPrompt = marker(
          'write',
          { path: firstOutput, content: proof },
          `native-channel-${nonce}-write`,
        )
        const firstMessage = inbound(firstPrompt, { photo: true })
        await reply(
          firstMessage,
          (text) => text.includes('需要审批：') && text.includes('/approve'),
          'Real channel tool must pause at the native permission boundary',
        )
        const first = await recordSession()
        const permission = (await channelFile()).scopes[`telegram:${firstPeer}`].pendingApprovalId
        assert.equal(typeof permission, 'string')
        const queuedPrompt = marker('read', { path: firstOutput }, `native-channel-${nonce}-queued`)
        const queuedMessage = inbound(queuedPrompt)
        const otherPrompt = marker(
          'read',
          { path: peerProof },
          `native-channel-${nonce}-other-peer`,
        )
        const otherMessage = inbound(otherPrompt, { peer: secondPeer })
        await reply(
          otherMessage,
          (text) => text.includes('Rust 流式聊天验收通过'),
          'A different peer must progress while the first peer awaits approval',
        )
        const other = await recordSession(secondPeer)
        assert.notEqual(other.sessionId, first.sessionId)
        assert.equal((await scope()).lastMessage, '')
        assert.equal(
          replies(queuedMessage).length,
          0,
          'Ordinary same-peer messages must remain queued behind the pending tool',
        )
        await assertTool(other.sessionId, 'read', `Native other peer proof ${nonce}`)
        const approvalMessage = inbound('/approve')
        await reply(
          approvalMessage,
          (text) => text.includes('已批准，继续执行。'),
          'Approval commands must bypass the same-peer work queue',
        )
        await reply(firstMessage, (text) => text.includes('Rust 流式聊天验收通过'))
        await reply(queuedMessage, (text) => text.includes('Rust 流式聊天验收通过'))
        await poll(
          () => scope(),
          (entry) => entry?.lastMessage === queuedPrompt.slice(0, 120),
          'Queued prompt must reuse and update the same durable channel scope',
        )
        assert.equal((await scope()).sessionId, first.sessionId)
        assert.equal(
          (await channelFile()).scopes[`telegram:${firstPeer}`].pendingApprovalId,
          undefined,
        )
        assert.equal(await readFile(firstOutput, 'utf8'), proof)
        await assertTool(first.sessionId, 'read', proof.trim())
        assert.ok(
          fixtureRequests.slice(modelStart).some((entry) => entry.imageCount === 1),
          'Actual downloaded Telegram PNG must reach the actual model as an image',
        )
        assert.ok(
          calls.some((call) => call.method === 'downloadReference' && call.sha256 === sha(png)),
        )
        const fileSends = await poll(
          () =>
            calls.filter(
              (call) => call.method === 'sendDocument' && call.name === basename(firstOutput),
            ),
          (rows) => rows.length === 1,
          'Real native generated original asset must be sent as a Telegram multipart document',
        )
        assert.equal(fileSends[0].peer, String(firstPeer))
        assert.deepEqual(fileSends[0].bytes, Buffer.from(proof))
        const assets = (await json(`/api/assets?sessionId=${first.sessionId}`)).assets
        for (const asset of assets) if (!initialAssetIds.has(asset.id)) ownedAssets.add(asset.id)
        const exported = assets.find(
          (asset) => asset.filePath && resolve(asset.filePath) === firstOutput,
        )
        assert.ok(exported, 'Native generated channel file must remain in public asset archive')
        const downloaded = await request(`/api/assets/${exported.id}/download`)
        assert.equal(downloaded.status, 200)
        assert.deepEqual(Buffer.from(await downloaded.arrayBuffer()), Buffer.from(proof))
        for (const route of ['live', 'messages?limit=200'])
          assert.ok(
            (await json(`/api/sessions/${first.sessionId}/${route}`)).messages.some(
              (message) =>
                message.role === 'agent' &&
                message.attachments?.some((asset) => asset.id === exported.id),
            ),
          )

        const denialStart = fixtureRequests.length
        const denied = inbound(
          marker('read', { path: peerProof }, 'unauthorized-channel-message'),
          { peer: deniedPeer, sender: owner + 1 },
        )
        await reply(denied, (text) => text === '当前机器人仅允许扫码创建者使用。')
        assert.equal(
          fixtureRequests.length,
          denialStart,
          'Denied senders must not start a model request',
        )
        assert.equal(await scope(deniedPeer), undefined)
        for (const [command, expected] of [
          ['/mode workspace', '审批模式已切换为 workspace-write'],
          ['/run team', '执行模式已切换为 team'],
          ['/run plan', '执行模式已切换为 plan'],
          [`/dir ${changedDirectory}`, '工作目录已切换为'],
          ['/status', '会话：'],
        ]) {
          const id = inbound(command)
          await reply(id, (text) => text.includes(expected))
        }
        const configuredScope = await scope()
        assert.equal(configuredScope.executionMode, 'workspace-write')
        assert.equal(configuredScope.runMode, 'plan')
        assert.equal(resolve(configuredScope.cwd), changedDirectory)
        const live = await json(`/api/sessions/${first.sessionId}/live`)
        assert.equal(live.executionMode, 'workspace-write')
        assert.equal(resolve(live.cwd), changedDirectory)

        const template = `native-channel-template-${nonce} {{chat.title}} / {{missing.future}}`
        templateChanged = true
        await json(templateRoute, 'PUT', { enabled: true, content: template })
        const notificationStart = calls.length
        const notification = await json(templateRoute + '/test', 'POST', {})
        assert.deepEqual(Object.keys(notification).sort(), ['preview', 'sent'])
        assert.equal(notification.sent, 1)
        assert.equal(typeof notification.preview, 'string')
        const notificationSend = calls
          .slice(notificationStart)
          .find((call) => call.method === 'sendMessage' && call.body.text === notification.preview)
        assert.ok(notificationSend, 'Template test must use the actual native Telegram transport')
        assert.equal(notificationSend.body.chat_id, String(firstPeer))
        assert.ok(
          notification.preview.includes(`native-channel-template-${nonce}`) &&
            notification.preview.includes('{{missing.future}}'),
        )
        await json(templateRoute, 'PUT', {
          enabled: initialTemplate.enabled,
          content: initialTemplate.channels.telegram.content,
        })
        templateChanged = false
        assert.deepEqual((await channelFile()).templates, baseline.templates)
        assert.deepEqual(await documents(), initialDocuments)
        assert.deepEqual(errors, [])
        await poll(
          () => calls.filter((call) => call.method === 'getUpdates').at(-1)?.body.offset,
          (offset) => offset > updateId,
          'Owned Telegram updates must be acknowledged before backend restart',
        )
        retained = {
          sessionId: first.sessionId,
          otherSessionId: other.sessionId,
          scope: configuredScope,
          generatedAssetId: exported.id,
          generatedSha256: sha(Buffer.from(proof)),
          inboundSha256: sha(png),
          getMeCalls: calls.filter((call) => call.method === 'getMe').length,
        }
        return {
          actualTelegramProtocol: true,
          actualNativeAgent: true,
          actualPermissionApprovedThroughChannel: true,
          separatePeerProgress: true,
          samePeerQueueOrdered: true,
          ownerDeniedBeforeModel: true,
          sessionId: first.sessionId,
          assetId: exported.id,
          assetSha256: retained.generatedSha256,
          templateTransportSent: true,
          commandsMutatedActualSession: true,
          providerAuthAppSettingsPreserved: true,
          retainedForRestart: true,
        }
      } finally {
        if (!retained) await cleanup()
      }
    },
  )

  if (!retained) return undefined
  const checkRestart = async () => {
    try {
      assert.ok(
        restartDocuments,
        'Capture the exact configuration after other fixture mutations and before backend stop',
      )
      assert.deepEqual(await documents(), restartDocuments)
      const state = await poll(
        () => json('/api/channels'),
        (value) => value.connections.telegram?.status === 'connected',
        'Backend restart must restore the actual local Telegram connection',
      )
      assert.ok(calls.filter((call) => call.method === 'getMe').length > retained.getMeCalls)
      assert.equal(
        state.scopes.find((entry) => entry.key === `telegram:${firstPeer}`).sessionId,
        retained.sessionId,
      )
      const restarted = await scope()
      assert.equal(restarted.executionMode, retained.scope.executionMode)
      assert.equal(restarted.runMode, retained.scope.runMode)
      assert.equal(resolve(restarted.cwd), changedDirectory)
      assert.equal(JSON.stringify(state).includes(token), false)
      const id = inbound(
        marker('read', { path: firstOutput }, `native-channel-${nonce}-after-restart`),
      )
      await reply(
        id,
        (text) => text.includes('Rust 流式聊天验收通过'),
        'Restored channel must perform another real native Agent tool turn',
      )
      assert.equal((await recordSession()).sessionId, retained.sessionId)
      await assertTool(retained.sessionId, 'read', proof.trim())
      assert.deepEqual(await documents(), restartDocuments)
      assert.deepEqual(errors, [])
      return {
        actualTelegramReconnected: true,
        scopeAndSessionPersisted: true,
        restartedAgentToolRoundtrip: true,
        providerAuthAppSettingsPreserved: true,
        generatedAssetSha256: retained.generatedSha256,
      }
    } finally {
      await cleanup()
    }
  }
  checkRestart.prepareRestart = async () => {
    assert.ok(retained && !cleaned, 'The owned channel fixture must be staged before restart')
    assert.equal(restartDocuments, undefined, 'The restart baseline must be captured exactly once')
    const current = await documents()
    for (const name of ['models.json', 'auth.json', 'settings.json']) {
      assert.equal(current[name], initialDocuments[name], `${name} must retain its original bytes`)
    }
    restartDocuments = current
    return { configurationSha256: restartDocuments }
  }
  checkRestart.dispose = cleanup
  return checkRestart
}
