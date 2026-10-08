import assert from 'node:assert/strict'
import { createHash, randomUUID } from 'node:crypto'
import {
  lstat,
  mkdir,
  readFile,
  readdir,
  realpath,
  rm,
  rmdir,
  unlink,
  writeFile,
} from 'node:fs/promises'
import { createServer } from 'node:http'
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path'
import jpeg from 'jpeg-js'
import { PNG } from 'pngjs'

const hash = (bytes) => createHash('sha256').update(bytes).digest('hex')
const statusKeys = [
  'image',
  'imageModels',
  'imageSelection',
  'video',
  'videoModels',
  'videoSelection',
]
const ownAppKeys = new Set([
  'enabledTools',
  'toolMode',
  'pluginChanges',
  'pluginsUpdatedAt',
  'visualDefaultModels',
])
const without = (value, keys) =>
  Object.fromEntries(Object.entries(value || {}).filter(([name]) => !keys.has(name)))

function inside(root, path) {
  const child = relative(root, path)
  return (
    child !== '' &&
    child !== '..' &&
    !child.startsWith('..' + '/') &&
    !child.startsWith('..' + '\\') &&
    !isAbsolute(child)
  )
}

function sourcePng() {
  const png = new PNG({ width: 16, height: 8 })
  for (let y = 0; y < png.height; y++) {
    for (let x = 0; x < png.width; x++) png.data.set([x * 13, y * 27, 110, 255], (y * 16 + x) * 4)
  }
  return PNG.sync.write(png)
}

// 完整 ISO BMFF 包含实际 JPEG 帧、采样表和时序，确保下载证据是可解码视频。
function videoFixture() {
  const pixels = PNG.sync.read(sourcePng())
  const sample = Buffer.from(jpeg.encode(pixels, 75).data)
  assert.equal(jpeg.decode(sample).width, 16)
  const u32 = (...values) => {
    const bytes = Buffer.alloc(values.length * 4)
    values.forEach((value, index) => bytes.writeUInt32BE(value, index * 4))
    return bytes
  }
  const box = (name, ...data) => {
    const body = Buffer.concat(data)
    return Buffer.concat([u32(body.length + 8), Buffer.from(name, 'ascii'), body])
  }
  const full = (name, flags, ...data) => box(name, u32(flags), ...data)
  const matrix = (bytes, offset) => {
    bytes.writeUInt32BE(0x10000, offset)
    bytes.writeUInt32BE(0x10000, offset + 16)
    bytes.writeUInt32BE(0x40000000, offset + 32)
  }
  const ftyp = box('ftyp', Buffer.from('isom'), u32(512), Buffer.from('isomiso2mp41'))
  const mdat = box('mdat', sample)
  const mvhd = Buffer.alloc(96)
  mvhd.writeUInt32BE(1000, 8)
  mvhd.writeUInt32BE(1000, 12)
  mvhd.writeUInt32BE(0x10000, 16)
  mvhd.writeUInt16BE(0x100, 20)
  matrix(mvhd, 32)
  mvhd.writeUInt32BE(2, 92)
  const tkhd = Buffer.alloc(80)
  tkhd.writeUInt32BE(1, 8)
  tkhd.writeUInt32BE(1000, 16)
  matrix(tkhd, 36)
  tkhd.writeUInt32BE(16 * 0x10000, 72)
  tkhd.writeUInt32BE(8 * 0x10000, 76)
  const mdhd = Buffer.alloc(20)
  mdhd.writeUInt32BE(1000, 8)
  mdhd.writeUInt32BE(1000, 12)
  mdhd.writeUInt16BE(0x55c4, 16)
  const entry = Buffer.alloc(78)
  entry.writeUInt16BE(1, 6)
  entry.writeUInt16BE(16, 24)
  entry.writeUInt16BE(8, 26)
  entry.writeUInt32BE(0x480000, 28)
  entry.writeUInt32BE(0x480000, 32)
  entry.writeUInt16BE(1, 40)
  entry.writeUInt16BE(24, 74)
  entry.writeUInt16BE(0xffff, 76)
  const stbl = box(
    'stbl',
    full('stsd', 0, u32(1), box('jpeg', entry)),
    full('stts', 0, u32(1, 1, 1000)),
    full('stsc', 0, u32(1, 1, 1, 1)),
    full('stsz', 0, u32(sample.length, 1)),
    full('stco', 0, u32(1, ftyp.length + 8)),
  )
  const minf = box(
    'minf',
    full('vmhd', 1, Buffer.alloc(8)),
    box('dinf', full('dref', 0, u32(1), full('url ', 1))),
    stbl,
  )
  const mdia = box(
    'mdia',
    full('mdhd', 0, mdhd),
    full('hdlr', 0, u32(0), Buffer.from('vide'), Buffer.alloc(12), Buffer.from('Native fixture\0')),
    minf,
  )
  return Buffer.concat([
    ftyp,
    mdat,
    box('moov', full('mvhd', 0, mvhd), box('trak', full('tkhd', 3, tkhd), mdia)),
  ])
}

// 调用方拥有 Rust 服务和聊天/图像夹具；此处仅监听新的本机视频夹具端口。
export async function checkVisualParity({
  check,
  json,
  request,
  chat,
  workspace,
  agent,
  output,
  providerId,
  modelId,
  imageProviderId,
  imageModelId,
  imageBaseUrl,
  imageFixtureRequests,
  fixtureRequests,
  delay,
}) {
  return await check(
    'Visual APIs and actual Pi discovery/gateway generate, edit and poll video with durable assets',
    async () => {
      const failures = []
      const sandbox = await realpath(resolve(output))
      const cwd = await realpath(resolve(workspace))
      const data = await realpath(resolve(agent))
      assert.ok(
        inside(sandbox, cwd) && inside(sandbox, data),
        'Use only the caller-owned synthetic sandbox',
      )
      assert.ok(Array.isArray(imageFixtureRequests) && Array.isArray(fixtureRequests))
      const imageOrigin = new URL(imageBaseUrl).origin
      assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(new URL(imageOrigin).hostname))
      const nonce = randomUUID()
      const fixture = join(cwd, `visual-fixture-${nonce}`)
      const videoProvider = `native-visual-video-${nonce}`
      const videoModel = 'sora-2-native-fixture'
      const videoId = `video-${nonce}`
      const videoKey = `synthetic-visual-${nonce}`
      const videoBytes = videoFixture()
      const videoRequests = []
      const generated = new Set()
      const assets = new Set()
      let server
      const serverErrors = []
      let initialApp
      let initialStatus
      let initialPlugins
      let initialHashes
      let credentialHashes
      let session
      let sessionJoined = true
      let createdFixture = false
      let createdProvider = false
      let preferencesChanged = false
      let toolsChanged = false
      const createdTestDirectories = []
      let testPath
      let evidence
      const app = async () => JSON.parse(await readFile(join(data, 'pisper.json'), 'utf8'))
      const hashes = async () =>
        Object.fromEntries(
          await Promise.all(
            ['models.json', 'auth.json'].map(async (name) => [
              name,
              hash(await readFile(join(data, name))),
            ]),
          ),
        )
      function response(path, method = 'GET', body) {
        return request(path, {
          method,
          ...(body === undefined
            ? {}
            : {
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
              }),
          signal: AbortSignal.timeout(20000),
        })
      }
      async function poll(read, predicate, label) {
        const deadline = Date.now() + 10000
        let latest
        while (Date.now() < deadline) {
          latest = await read()
          if (predicate(latest)) return latest
          await delay(25)
        }
        assert.fail(`${label}: ${JSON.stringify(latest)}`)
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
          'Owned configuration/session mutation must settle',
        )
      }
      function setTools(enabledTools) {
        return idleJson('/api/plugins', 'PUT', {
          enabledTools,
          webSearch: initialPlugins.webSearch,
          piExtensions: initialPlugins.piExtensions,
          computerUseEnabled: initialPlugins.computerUseEnabled,
        })
      }
      async function invoke(name, args) {
        const startIndex = fixtureRequests.length
        const marker =
          'rust-snapshot-tool:' + Buffer.from(JSON.stringify({ name, args })).toString('base64')
        const events = await chat(`${marker} native-visual-${nonce}`, session.id)
        const start = events.find(
          (event) => event.event === 'tool_start' && event.data.name === name,
        )
        assert.ok(start, `Real Pi ${name} must start`)
        const end = events.find(
          (event) => event.event === 'tool_end' && event.data.id === start.data.id,
        )
        assert.ok(end, `Real Pi ${name} must finish`)
        assert.equal(end.data.error, false, JSON.stringify(end.data.result))
        const actual = fixtureRequests.slice(startIndex)
        assert.ok(
          actual.some((entry) => entry.tools.includes(name)),
          'The actual model request must expose this gateway tool',
        )
        assert.ok(
          actual.some((entry) =>
            entry.toolResults.some(
              (result) => result.tool_call_id === start.data.id && result.text,
            ),
          ),
          'The real tool result must return to the model',
        )
        const history = await json(`/api/sessions/${session.id}/messages?limit=200`)
        assert.ok(
          history.messages.some(
            (message) =>
              message.role === 'agent' &&
              message.runActivity?.tools?.some(
                (tool) =>
                  tool.id === start.data.id &&
                  tool.name === name &&
                  tool.status === 'done' &&
                  typeof tool.output === 'string' &&
                  tool.output.length > 0,
              ),
          ),
          'Restored public tool activity must retain the completed tool-call identity and output',
        )
        return { result: end.data.result, events }
      }
      async function visual(args) {
        const { result } = await invoke('call_tool', { name: 'generate_visual', arguments: args })
        assert.equal(result.details.gatewayToolName, 'generate_visual')
        const { gatewayToolName: _gateway, ...details } = result.details
        assert.equal(details.kind, args.kind)
        assert.equal(details.operation, args.sourceImages ? 'edit' : 'generate')
        assert.equal(details.fallbackUsed, false)
        assert.deepEqual(details.attemptedModels, [args.model])
        assert.equal(typeof details.providerName, 'string')
        assert.equal(typeof details.modelName, 'string')
        assert.equal(details.provider + '/' + details.model, args.model)
        const path = await realpath(details.path)
        assert.equal(dirname(path), join(fixture, 'generated', 'visuals'))
        assert.ok(inside(fixture, path), 'Generated files must use the actual native session cwd')
        generated.add(path)
        const bytes = await readFile(path)
        assert.equal(bytes.length, details.size)
        if (args.kind === 'image') {
          const png = PNG.sync.read(bytes)
          assert.equal(png.width, 8)
          assert.equal(png.height, 4)
          assert.equal(details.mimeType, 'image/png')
          assert.equal(details.remoteId, null)
        } else {
          assert.equal(details.remoteId, videoId)
          assert.equal(details.mimeType, 'video/mp4')
          assert.deepEqual(bytes, videoBytes)
          assert.equal(bytes.subarray(4, 8).toString(), 'ftyp')
          assert.ok(bytes.includes(Buffer.from('moov')) && bytes.includes(Buffer.from('jpeg')))
        }
        const text = result.content.find((part) => part.type === 'text')?.text
        assert.ok(
          text?.includes(`File: ${details.path}`) && text.includes(`Model: ${details.modelName}`),
        )
        const archived = (await json(`/api/assets?sessionId=${session.id}`)).assets
        const matches = archived.filter(
          (asset) => asset.filePath && resolve(asset.filePath) === path,
        )
        assert.equal(matches.length, 1, 'The generated original file must be indexed once')
        const asset = matches[0]
        assets.add(asset.id)
        assert.equal(asset.sessionId, session.id)
        assert.equal(asset.sessionName, session.name)
        assert.equal(asset.source, 'agent')
        assert.equal(asset.mimeType, details.mimeType)
        const download = await request(`/api/assets/${asset.id}/download`)
        assert.equal(download.status, 200)
        assert.equal(download.headers.get('content-type')?.split(';')[0], details.mimeType)
        assert.deepEqual(Buffer.from(await download.arrayBuffer()), bytes)
        for (const route of ['live', 'messages?limit=200']) {
          const history = await json(`/api/sessions/${session.id}/${route}`)
          assert.ok(
            history.messages.some(
              (message) =>
                message.role === 'agent' &&
                message.attachments?.some((attachment) => attachment.id === asset.id),
            ),
            'Both live and restored history must expose the actual generated asset',
          )
        }
        return { ...details, sha256: hash(bytes), assetId: asset.id }
      }
      try {
        initialApp = await app()
        initialStatus = await json('/api/visual/models')
        initialPlugins = await json('/api/plugins')
        initialHashes = await hashes()
        assert.deepEqual(Object.keys(initialStatus).sort(), [...statusKeys].sort())
        assert.ok(Array.isArray(initialApp.enabledTools))
        for (const kind of ['image', 'video']) {
          const previous = initialApp.visualDefaultModels?.[kind]
          assert.ok(
            previous === undefined ||
              (typeof previous === 'string' &&
                previous.length > 0 &&
                previous === initialStatus[kind + 'Selection']),
            'Only a preference restorable through the public API may be temporarily changed',
          )
        }
        assert.ok(
          initialStatus.imageModels.some(
            (model) => model.providerId === imageProviderId && model.id === imageModelId,
          ),
        )
        const providers = JSON.parse(await readFile(join(data, 'models.json'), 'utf8')).providers
        const configured = providers[imageProviderId]
        const model = configured.models.find((item) => item.id === imageModelId)
        assert.equal(new URL(model.baseUrl || configured.baseUrl).origin, imageOrigin)
        const chatProvider = providers[providerId]
        const chatModel = chatProvider.models.find((item) => item.id === modelId)
        assert.ok(chatModel, 'Use only the caller-owned synthetic chat model')
        const chatUrl = new URL(chatModel.baseUrl || chatProvider.baseUrl)
        assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(chatUrl.hostname))
        for (const entry of [...initialStatus.imageModels, ...initialStatus.videoModels]) {
          assert.equal(Object.hasOwn(entry, 'apiKey'), false)
          assert.equal(Object.hasOwn(entry, 'headers'), false)
          assert.ok(
            ['127.0.0.1', 'localhost', '[::1]'].includes(new URL(entry.baseUrl).hostname),
            'Automatic configuration-test fallback must remain within loopback providers',
          )
        }
        server = createServer(async (req, res) => {
          try {
            assert.ok(
              req.headers.authorization === `Bearer ${videoKey}`,
              'Use only the owned synthetic video credential',
            )
            const path = new URL(req.url, 'http://127.0.0.1')
            const record = {
              method: req.method,
              path: path.pathname,
              query: path.search,
              authenticated: true,
            }
            if (req.method === 'POST' && path.pathname === '/v1/videos') {
              assert.match(req.headers['content-type'], /^multipart\/form-data;/)
              const chunks = []
              for await (const chunk of req) chunks.push(chunk)
              const body = Buffer.concat(chunks)
              assert.ok(body.length < 1024 * 1024, 'The controlled create request is bounded')
              const form = await new Response(body, {
                headers: { 'Content-Type': req.headers['content-type'] },
              }).formData()
              record.fields = Object.fromEntries(form)
              assert.deepEqual(record.fields, {
                model: videoModel,
                prompt: `native-visual-${nonce}-video`,
                seconds: '4',
                size: '1280x720',
              })
              videoRequests.push(record)
              res.writeHead(200, { 'Content-Type': 'application/json' })
              res.end(JSON.stringify({ id: videoId, status: 'queued', progress: 0 }))
            } else if (req.method === 'GET' && path.pathname === `/v1/videos/${videoId}`) {
              videoRequests.push(record)
              res.writeHead(200, { 'Content-Type': 'application/json' })
              res.end(JSON.stringify({ id: videoId, status: 'completed', progress: 100 }))
            } else if (req.method === 'GET' && path.pathname === `/v1/videos/${videoId}/content`) {
              assert.equal(path.searchParams.get('variant'), 'video')
              videoRequests.push(record)
              res.writeHead(200, {
                'Content-Type': 'video/mp4',
                'Content-Length': videoBytes.length,
              })
              res.end(videoBytes)
            } else assert.fail(`Unexpected owned video request ${req.method} ${path.pathname}`)
          } catch (error) {
            serverErrors.push(error)
            if (!res.headersSent) res.writeHead(500, { 'Content-Type': 'application/json' })
            res.end(
              JSON.stringify({
                error: { message: 'Controlled video fixture rejected the request' },
              }),
            )
          }
        })
        await new Promise((done, reject) => {
          server.once('error', reject)
          server.listen(0, '127.0.0.1', done)
        })
        createdProvider = true
        const created = await idleJson(
          '/api/providers',
          'POST',
          {
            id: videoProvider,
            name: `Native Visual Video ${nonce}`,
            providerType: 'visual',
            api: 'openai-completions',
            baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
            apiKey: videoKey,
            model: videoModel,
            modelKind: 'video',
            enabled: true,
          },
          201,
        )
        assert.equal(created.createdProviderId, videoProvider)
        credentialHashes = await hashes()
        const beforePreferences = await app()
        preferencesChanged = true
        for (const [kind, selected] of [
          ['image', `${imageProviderId}/${imageModelId}`],
          ['video', `${videoProvider}/${videoModel}`],
        ]) {
          const changed = await json(`/api/visual/models/${kind}`, 'PUT', { model: selected })
          assert.deepEqual(Object.keys(changed).sort(), [...statusKeys].sort())
          assert.equal(changed[kind + 'Selection'], selected)
          assert.equal(changed[kind].providerId + '/' + changed[kind].id, selected)
          assert.equal((await app()).visualDefaultModels[kind], selected)
          const reset = await json(`/api/visual/models/${kind}`, 'PUT', { model: null })
          assert.equal(reset[kind + 'Selection'], '')
          assert.equal(Object.hasOwn((await app()).visualDefaultModels, kind), false)
        }
        assert.deepEqual(without(await app(), ownAppKeys), without(beforePreferences, ownAppKeys))
        assert.deepEqual(await hashes(), credentialHashes)
        const beforeInvalid = hash(await readFile(join(data, 'pisper.json')))
        for (const [path, status, body] of [
          ['/api/visual/models/audio', 404, { model: null }],
          ['/api/visual/models/image', 400, { model: `missing-${nonce}` }],
        ]) {
          const result = await response(path, 'PUT', body)
          const error = await result.json()
          assert.equal(result.status, status)
          assert.equal(typeof error.error, 'string')
          assert.equal(Object.keys(error).length, 1)
        }
        assert.equal(hash(await readFile(join(data, 'pisper.json'))), beforeInvalid)
        await json('/api/visual/models/image', 'PUT', {
          model: `${imageProviderId}/${imageModelId}`,
        })
        const testDirectory = join(data, 'visual-test', 'generated', 'visuals')
        for (const directory of [
          join(data, 'visual-test'),
          join(data, 'visual-test', 'generated'),
          testDirectory,
        ]) {
          const info = await lstat(directory).catch((error) => {
            if (error.code === 'ENOENT') return null
            throw error
          })
          if (info) {
            assert.ok(info.isDirectory() && !info.isSymbolicLink())
            assert.equal(await realpath(directory), directory)
          } else createdTestDirectories.push(directory)
        }
        const priorTestFiles = new Set(
          await readdir(testDirectory).catch((error) => {
            if (error.code === 'ENOENT') return []
            throw error
          }),
        )
        const testStart = imageFixtureRequests.length
        const tested = await json('/api/visual/test', 'POST', {
          prompt: 'ignored',
          kind: 'video',
          cwd: fixture,
        })
        const actualTestPath = await realpath(tested.path)
        assert.equal(dirname(actualTestPath), testDirectory)
        assert.ok(
          !inside(cwd, actualTestPath),
          'Config-test output belongs to data/visual-test, not the workspace',
        )
        assert.ok(
          !priorTestFiles.has(basename(actualTestPath)),
          'Config-test must not overwrite an older fixture during this run',
        )
        testPath = actualTestPath
        const testBytes = await readFile(testPath)
        assert.equal(PNG.sync.read(testBytes).width, 8)
        assert.equal(tested.previewDataUrl, `data:image/png;base64,${testBytes.toString('base64')}`)
        assert.equal(tested.kind, 'image')
        assert.equal(tested.operation, 'generate')
        assert.equal(tested.provider + '/' + tested.model, `${imageProviderId}/${imageModelId}`)
        assert.equal(tested.mimeType, 'image/png')
        assert.equal(tested.size, testBytes.length)
        assert.equal(tested.fallbackUsed, false)
        assert.deepEqual(tested.attemptedModels, [`${imageProviderId}/${imageModelId}`])
        const testCalls = imageFixtureRequests.slice(testStart)
        assert.equal(testCalls.length, 1)
        assert.equal(testCalls[0].path, '/v1/images/generations')
        assert.equal(testCalls[0].model, imageModelId)
        assert.equal(testCalls[0].multipart, false)
        assert.equal(
          testCalls[0].prompt,
          'a small friendly robot mascot waving, flat vector illustration, soft pastel colors, plain background',
        )
        toolsChanged = true
        await setTools([
          ...new Set([
            ...initialApp.enabledTools,
            ...initialPlugins.enabledTools,
            'generate_visual',
          ]),
        ])
        await mkdir(fixture)
        createdFixture = true
        const reference = sourcePng()
        await writeFile(join(fixture, 'source.png'), reference)
        session = await json('/api/sessions', 'POST', { name: `visual-${nonce}`, cwd: fixture })
        sessionJoined = false
        await json(`/api/sessions/${session.id}/model`, 'PUT', {
          provider: providerId,
          model: modelId,
        })
        await json(`/api/sessions/${session.id}/execution-mode`, 'PUT', { mode: 'workspace-write' })
        const live = await json(`/api/sessions/${session.id}/live`)
        assert.equal(resolve(live.cwd), fixture)
        assert.equal(live.executionMode, 'workspace-write')
        const discovery = (
          await invoke('discover_tools', { query: 'generate_visual image video', limit: 5 })
        ).result
        assert.ok(discovery.details.matches.some((match) => match.name === 'generate_visual'))
        const imageStart = imageFixtureRequests.length
        const image = await visual({
          kind: 'image',
          prompt: `native-visual-${nonce}-generate`,
          model: `${imageProviderId}/${imageModelId}`,
          outputName: `${nonce}-generated`,
        })
        const edited = await visual({
          kind: 'image',
          prompt: `native-visual-${nonce}-edit`,
          model: `${imageProviderId}/${imageModelId}`,
          sourceImages: ['source.png'],
          outputName: `${nonce}-edited`,
          outputFormat: 'png',
        })
        const calls = imageFixtureRequests.slice(imageStart)
        assert.equal(calls.length, 2)
        assert.equal(calls[0].path, '/v1/images/generations')
        assert.equal(calls[0].multipart, false)
        assert.equal(calls[1].path, '/v1/images/edits')
        assert.equal(calls[1].multipart, true)
        assert.deepEqual(calls[1].images, [
          { name: 'image', mimeType: 'image/png', size: reference.length, width: 16, height: 8 },
        ])
        const video = await visual({
          kind: 'video',
          prompt: `native-visual-${nonce}-video`,
          model: `${videoProvider}/${videoModel}`,
          durationSeconds: 4,
          size: '1280x720',
          outputName: `${nonce}-video`,
        })
        assert.deepEqual(
          videoRequests.map((entry) => entry.method + ' ' + entry.path),
          ['POST /v1/videos', `GET /v1/videos/${videoId}`, `GET /v1/videos/${videoId}/content`],
        )
        assert.deepEqual(serverErrors, [])
        assert.deepEqual(await hashes(), credentialHashes)
        evidence = {
          sessionId: session.id,
          actualPiGateway: true,
          discovery: 'generate_visual',
          images: [image, edited],
          video: { ...video, codec: 'single-frame MJPEG', requests: videoRequests },
          configTest: { path: testPath, sha256: hash(testBytes), previewMatchesDisk: true },
          preferencesPersistedAndReset: true,
          unknownAppFieldsPreserved: true,
          credentialsPreserved: true,
        }
      } catch (error) {
        failures.push(error)
      } finally {
        const cleanup = async (action) => {
          try {
            await action()
          } catch (error) {
            failures.push(error)
          }
        }
        if (session)
          await cleanup(async () => {
            const stopped = await response(`/api/sessions/${session.id}/abort`, 'POST', {})
            assert.ok([200, 404].includes(stopped.status))
            await poll(
              async () => {
                const result = await response(`/api/sessions/${session.id}`, 'DELETE')
                if (result.status === 409) return false
                assert.ok([200, 404].includes(result.status))
                return true
              },
              Boolean,
              'The owned visual session must be joined and removed',
            )
            sessionJoined = true
          })
        for (const id of assets)
          await cleanup(async () => {
            const deleted = await response(`/api/assets/${id}`, 'DELETE')
            assert.ok([200, 404].includes(deleted.status))
          })
        if (preferencesChanged)
          for (const kind of ['image', 'video'])
            await cleanup(async () => {
              await idleJson(`/api/visual/models/${kind}`, 'PUT', {
                model: initialStatus[kind + 'Selection'],
              })
            })
        if (toolsChanged)
          await cleanup(async () => {
            await setTools([
              ...new Set([...initialApp.enabledTools, ...initialPlugins.enabledTools]),
            ])
            assert.deepEqual((await app()).enabledTools, initialApp.enabledTools)
            assert.equal((await app()).toolMode, initialApp.toolMode)
            assert.deepEqual((await json('/api/plugins')).enabledTools, initialPlugins.enabledTools)
          })
        if (createdProvider)
          await cleanup(async () => {
            await poll(
              async () => {
                const result = await response(`/api/providers/${videoProvider}`, 'DELETE')
                if (result.status === 409) return false
                assert.ok([200, 404].includes(result.status))
                return true
              },
              Boolean,
              'The owned video provider must be removed after real runs settle',
            )
            assert.deepEqual(
              await hashes(),
              initialHashes,
              'All pre-existing provider/configuration bytes must be restored',
            )
          })
        if (initialApp && initialStatus)
          await cleanup(async () => {
            assert.deepEqual(without(await app(), ownAppKeys), without(initialApp, ownAppKeys))
            const selections = new Set(['image', 'video'])
            assert.deepEqual(
              without((await app()).visualDefaultModels, selections),
              without(initialApp.visualDefaultModels, selections),
            )
            for (const kind of selections) {
              assert.equal(
                (await app()).visualDefaultModels?.[kind],
                initialApp.visualDefaultModels?.[kind],
              )
              assert.equal(
                (await json('/api/visual/models'))[kind + 'Selection'],
                initialStatus[kind + 'Selection'],
              )
            }
          })
        if (server)
          await cleanup(async () => {
            server.closeAllConnections()
            await new Promise((done, reject) =>
              server.close((error) => (error ? reject(error) : done())),
            )
            assert.equal(server.listening, false)
          })
        if (createdFixture && sessionJoined)
          await cleanup(async () => {
            const info = await lstat(fixture)
            assert.ok(info.isDirectory() && !info.isSymbolicLink())
            const actual = await realpath(fixture)
            assert.equal(dirname(actual), cwd)
            assert.equal(basename(actual), `visual-fixture-${nonce}`)
            await rm(actual, { recursive: true })
          })
        if (testPath && sessionJoined)
          await cleanup(async () => {
            assert.ok(inside(join(data, 'visual-test'), testPath))
            assert.equal(
              dirname(await realpath(testPath)),
              join(data, 'visual-test', 'generated', 'visuals'),
            )
            await unlink(testPath)
          })
        if (sessionJoined)
          for (const directory of [...createdTestDirectories].reverse())
            await cleanup(async () => {
              const info = await lstat(directory).catch((error) => {
                if (error.code === 'ENOENT') return null
                throw error
              })
              if (!info) return
              assert.ok(info.isDirectory() && !info.isSymbolicLink())
              assert.ok(inside(data, directory))
              assert.equal(await realpath(directory), directory)
              await rmdir(directory)
            })
        if (serverErrors.length) failures.push(...serverErrors)
      }
      if (failures.length)
        throw new AggregateError(
          failures,
          failures.map((error) => error.stack || String(error)).join('\n'),
        )
      return {
        ...evidence,
        ownedSessionProviderFilesAndVideoServerCleaned: true,
        generatedFiles: generated.size,
      }
    },
  )
}
