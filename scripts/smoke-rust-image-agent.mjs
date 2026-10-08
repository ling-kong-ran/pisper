import assert from 'node:assert/strict'
import { createHash, randomUUID } from 'node:crypto'
import { lstat, mkdir, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises'
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path'
import { PNG } from 'pngjs'
import { parseImageOutput } from '../shared/image/image-operations.mjs'

const sha = (bytes) => createHash('sha256').update(bytes).digest('hex')
const terminal = new Set(['completed', 'failed', 'error', 'interrupted'])
const mutablePluginKeys = new Set([
  'enabledTools',
  'toolMode',
  'pluginChanges',
  'pluginsUpdatedAt',
  'webSearch',
  'piExtensions',
  'computerUseEnabled',
])

function unrelatedApp(value) {
  return Object.fromEntries(Object.entries(value).filter(([key]) => !mutablePluginKeys.has(key)))
}
function inside(root, path) {
  const suffix = relative(root, path)
  return (
    suffix !== '' &&
    suffix !== '..' &&
    !suffix.startsWith('../') &&
    !suffix.startsWith('..\\') &&
    !isAbsolute(suffix)
  )
}
function sourcePng() {
  const png = new PNG({ width: 32, height: 16 })
  for (let y = 0; y < png.height; y++) {
    for (let x = 0; x < png.width; x++) {
      const frame = Math.floor(x / 16)
      const opaque = x % 16 >= 5 && x % 16 <= 10 && y >= 3 && y <= 12
      png.data.set(
        opaque ? [40 + frame * 7, 70, 140, 255] : [255, 255, 255, 255],
        (y * png.width + x) * 4,
      )
    }
  }
  return PNG.sync.write(png)
}
function resultData(result) {
  assert.equal(result?.details?.gatewayToolName, 'image_assets', JSON.stringify(result))
  const { gatewayToolName: _gatewayName, ...details } = result.details
  const text = result.content.find((part) => part.type === 'text')?.text
  assert.equal(typeof text, 'string')
  assert.deepEqual(
    JSON.parse(text),
    details,
    'The model-visible content must contain the real image operation result',
  )
  const output = parseImageOutput(details.output)
  assert.deepEqual(output, details.output)
  return { ...details, output }
}

// Caller owns the sidecar, loopback models and isolated data directory. This
// helper starts no process and never invokes a domain service or mocks a route.
export async function checkImageAgentParity({
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
  const sandbox = await realpath(resolve(output))
  const cwd = await realpath(resolve(workspace))
  const data = await realpath(resolve(agent))
  assert.ok(
    inside(sandbox, cwd) && inside(sandbox, data),
    'Only the caller-owned sandbox may be used',
  )
  assert.ok(
    Array.isArray(imageFixtureRequests) && Array.isArray(fixtureRequests),
    'Pass actual incoming loopback model request records',
  )
  const nonce = randomUUID()
  const fixture = join(cwd, `image-agent-fixture-${nonce}`)
  const sourceName = `reference-${nonce}.png`
  const mediaRoot = join(data, 'image-tools-agent', 'workflow-media')
  const sessions = new Set()
  const assets = new Set()
  const mediaIds = new Set()
  let before
  let originalApp
  let providerHashes
  let parent
  let child
  let createdFixture = false
  let evidence

  function response(path, method, body) {
    return request(path, {
      method,
      ...(body === undefined
        ? {}
        : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(15000),
    })
  }
  async function poll(read, predicate, label, timeout = 10000) {
    const deadline = Date.now() + timeout
    let last
    while (Date.now() < deadline) {
      last = await read()
      if (predicate(last)) return last
      await delay(25)
    }
    assert.fail(`${label} did not settle: ${JSON.stringify(last)}`)
  }
  async function hashes() {
    return Object.fromEntries(
      await Promise.all(
        ['models.json', 'auth.json'].map(async (name) => [
          name,
          sha(await readFile(join(data, name))),
        ]),
      ),
    )
  }
  async function app() {
    return JSON.parse(await readFile(join(data, 'pisper.json'), 'utf8'))
  }
  function setTools(enabledTools) {
    return poll(
      async () => {
        const result = await response('/api/plugins', 'PUT', {
          enabledTools,
          webSearch: before.webSearch,
          piExtensions: before.piExtensions,
          computerUseEnabled: before.computerUseEnabled,
        })
        if (result.status === 409) return null
        const value = await result.json()
        assert.equal(result.status, 200, JSON.stringify(value))
        return value
      },
      Boolean,
      'Configuration write after actual runs have finished',
    )
  }
  const prompt = (arguments_) =>
    'rust-snapshot-tool:' +
    Buffer.from(
      JSON.stringify({
        name: 'call_tool',
        args: { name: 'image_assets', arguments: arguments_ },
      }),
    ).toString('base64')

  async function remember(result) {
    for (const reference of [
      ...result.output.frames.map((frame) => frame.media),
      ...(result.output.atlas ? [result.output.atlas.media] : []),
    ]) {
      assert.match(reference.id, /^[0-9a-f-]{36}$/)
      mediaIds.add(reference.id)
      const metadata = JSON.parse(
        await readFile(join(mediaRoot, reference.id, 'metadata.json'), 'utf8'),
      )
      const bytes = await readFile(join(mediaRoot, reference.id, 'data.bin'))
      assert.equal(metadata.version, 1)
      assert.deepEqual(metadata.media, reference)
      assert.equal(metadata.sha256, sha(bytes))
      assert.equal(bytes.length, reference.size)
    }
  }
  async function invoke(arguments_, failed = false) {
    const events = await chat(prompt(arguments_), parent.id)
    const start = events.find(
      (event) => event.event === 'tool_start' && event.data.name === 'call_tool',
    )
    assert.ok(
      start,
      `Actual call_tool did not start for ${arguments_.operation}: ${JSON.stringify(events)}`,
    )
    const end = events.find(
      (event) => event.event === 'tool_end' && event.data.id === start.data.id,
    )
    assert.ok(end, `Actual call_tool did not finish for ${arguments_.operation}`)
    assert.equal(end.data.error, failed, `${arguments_.operation}: ${JSON.stringify(end)}`)
    if (failed) {
      assert.match(
        JSON.stringify(end.data.result || end.data),
        /image_tools_agent_disabled|unavailable|停用|disabled/i,
      )
      return { events }
    }
    const result = resultData(end.data.result)
    await remember(result)
    return { result, events }
  }
  async function pixels(reference) {
    return PNG.sync.read(await readFile(join(mediaRoot, reference.id, 'data.bin')))
  }
  async function exported(result, expectedOwner, requireHistory = true) {
    assert.equal(result.files.length, 2)
    assert.deepEqual(
      result.files.map((file) => file.mimeType),
      ['image/png', 'application/json'],
    )
    const paths = await Promise.all(result.files.map((file) => realpath(file.path)))
    const ownRoot = await realpath(fixture)
    for (const path of paths) {
      assert.ok(inside(ownRoot, path), 'Actual execution cwd must own the export')
      assert.match(
        relative(ownRoot, path).replaceAll('\\', '/'),
        /^generated\/image-assets\/[0-9a-f-]{36}\/(?:atlas\.png|frames\.json)$/,
      )
    }
    assert.equal(dirname(paths[0]), dirname(paths[1]))
    const png = await readFile(paths[0])
    assert.deepEqual(png, await readFile(join(mediaRoot, result.output.atlas.media.id, 'data.bin')))
    const portableText = await readFile(paths[1], 'utf8')
    const portable = JSON.parse(portableText)
    assert.deepEqual(portable, {
      image: 'atlas.png',
      width: result.output.atlas.width,
      height: result.output.atlas.height,
      frames: result.output.atlas.frames,
    })
    assert.doesNotMatch(portableText, /"media"|"mimeType"|"id"/)
    const archived = (await json(`/api/assets?sessionId=${encodeURIComponent(expectedOwner)}`))
      .assets
    const owned = []
    for (let index = 0; index < paths.length; index++) {
      const matches = []
      for (const asset of archived) {
        if (asset.filePath && (await realpath(asset.filePath)) === paths[index]) matches.push(asset)
      }
      assert.equal(
        matches.length,
        1,
        `A real export must be archived once under owner ${expectedOwner}`,
      )
      const asset = matches[0]
      assets.add(asset.id)
      assert.equal(asset.source, 'agent')
      assert.equal(asset.sessionId, expectedOwner)
      assert.equal(asset.sessionName, parent.name)
      assert.equal(asset.mimeType, result.files[index].mimeType)
      const download = await request(`/api/assets/${encodeURIComponent(asset.id)}/download`)
      assert.equal(download.status, 200)
      assert.equal(download.headers.get('content-type')?.split(';')[0], asset.mimeType)
      assert.deepEqual(Buffer.from(await download.arrayBuffer()), await readFile(paths[index]))
      owned.push(asset.id)
    }
    if (requireHistory) {
      for (const route of [
        `/api/sessions/${expectedOwner}/live`,
        `/api/sessions/${expectedOwner}/messages?limit=200`,
      ]) {
        const history = await json(route)
        for (const id of owned)
          assert.ok(
            history.messages.some(
              (message) =>
                message.role === 'agent' &&
                message.attachments?.some((attachment) => attachment.id === id),
            ),
            `Owner history must restore exported asset ${id}`,
          )
      }
    }
    return {
      directory: basename(dirname(paths[0])),
      atlasHash: sha(png),
      bytes: png.length,
      assetIds: owned,
    }
  }
  async function safeRemove(parentPath, path, expected) {
    let info
    try {
      info = await lstat(path)
    } catch (error) {
      if (error.code === 'ENOENT') return
      throw error
    }
    assert.ok(info.isDirectory() && !info.isSymbolicLink())
    const actual = await realpath(path)
    assert.equal(dirname(actual), await realpath(parentPath))
    assert.equal(basename(actual), expected)
    await rm(actual, { recursive: true })
  }
  async function cleanup() {
    const failures = []
    for (const id of sessions) {
      try {
        const abort = await response(`/api/sessions/${id}/abort`, 'POST', {})
        assert.ok([200, 404].includes(abort.status))
        await poll(
          async () => {
            const deleted = await response(`/api/sessions/${id}`, 'DELETE')
            if (deleted.status === 409) return false
            if (deleted.status === 404) return true
            const result = await deleted.json()
            assert.equal(deleted.status, 200, JSON.stringify(result))
            assert.equal(result.deleted, true)
            return true
          },
          Boolean,
          'Delete owned image session and join its private child',
        )
      } catch (error) {
        failures.push(error)
      }
    }
    if (before && originalApp) {
      try {
        await setTools([...new Set([...originalApp.enabledTools, ...before.enabledTools])])
        assert.deepEqual((await app()).enabledTools, originalApp.enabledTools)
        assert.deepEqual(unrelatedApp(await app()), unrelatedApp(originalApp))
        assert.deepEqual(await hashes(), providerHashes)
      } catch (error) {
        failures.push(error)
      }
    }
    for (const id of assets) {
      try {
        assert.equal((await json(`/api/assets/${encodeURIComponent(id)}`, 'DELETE')).deleted, true)
      } catch (error) {
        failures.push(error)
      }
    }
    if (failures.length === 0) {
      for (const id of mediaIds) {
        try {
          await safeRemove(mediaRoot, join(mediaRoot, id), id)
        } catch (error) {
          failures.push(error)
        }
      }
      if (createdFixture) {
        try {
          await safeRemove(cwd, fixture, basename(fixture))
        } catch (error) {
          failures.push(error)
        }
      }
    }
    if (failures.length) throw new AggregateError(failures, 'Image Agent owned cleanup failed')
  }

  await check(
    'Actual Pi image_assets uses native pixels and provider HTTP, archives public exports and rejects undelegated child calls',
    async () => {
      const failures = []
      try {
        before = await json('/api/plugins')
        originalApp = await app()
        assert.ok(
          Array.isArray(originalApp.enabledTools),
          'The initialized canonical app must expose the original enabledTools array',
        )
        providerHashes = await hashes()
        const config = await json('/api/config')
        const visual = config.providers.find((provider) => provider.id === imageProviderId)
        assert.ok(visual?.configured && visual.enabled)
        const fixtureUrl = new URL(imageBaseUrl)
        assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(fixtureUrl.hostname))
        assert.ok(
          ['127.0.0.1', 'localhost', '[::1]'].includes(new URL(visual.baseUrl).hostname),
          'Generation must target only the existing loopback image provider',
        )
        assert.ok(
          visual.models.some((model) => model.id === imageModelId && model.kind === 'image'),
        )
        const configuredModels = JSON.parse(await readFile(join(data, 'models.json'), 'utf8'))
        const definition = configuredModels.providers[imageProviderId]
        const configured = definition.models.find((model) => model.id === imageModelId)
        assert.ok(configured)
        assert.equal(
          new URL(configured.baseUrl || definition.baseUrl || visual.baseUrl).origin,
          fixtureUrl.origin,
          'A model-level override must also target the owned loopback fixture',
        )
        await setTools([
          ...new Set([...originalApp.enabledTools, ...before.enabledTools, 'image_assets']),
        ])
        for (const name of originalApp.enabledTools)
          assert.ok(
            (await app()).enabledTools.includes(name),
            `Enabling image_assets must preserve ${name}`,
          )
        await mkdir(fixture)
        createdFixture = true
        const source = sourcePng()
        await writeFile(join(fixture, sourceName), source)
        parent = await json('/api/sessions', 'POST', { name: `image-agent-${nonce}`, cwd: fixture })
        sessions.add(parent.id)
        await json(`/api/sessions/${parent.id}/model`, 'PUT', {
          provider: providerId,
          model: modelId,
        })
        await json(`/api/sessions/${parent.id}/execution-mode`, 'PUT', { mode: 'workspace-write' })
        const live = await json(`/api/sessions/${parent.id}/live`)
        assert.equal(resolve(live.cwd), fixture)
        assert.equal(live.executionMode, 'workspace-write')
        const input = (await invoke({ operation: 'input', sourceImage: sourceName })).result
        assert.equal(input.output.frames[0].media.name, sourceName)
        assert.equal(input.output.frames[0].width, 32)
        assert.equal(input.output.frames[0].height, 16)
        assert.deepEqual(
          await readFile(join(mediaRoot, input.output.frames[0].media.id, 'data.bin')),
          source,
        )
        for (const prefix of ['/api/game-assets/media', '/api/workflow-media']) {
          const isolated = await request(`${prefix}/${input.output.frames[0].media.id}/content`)
          assert.equal(isolated.status, 404, 'Agent media must not become a game/workflow resource')
          assert.equal((await isolated.json()).code, 'workflow_media_missing')
        }
        const background = (
          await invoke({
            operation: 'background',
            images: input.output.frames,
            settings: { method: 'color', colors: ['#ffffff'], tolerance: 0, softness: 0 },
          })
        ).result
        const removed = await pixels(background.output.frames[0].media)
        assert.equal(removed.data[3], 0)
        assert.equal(removed.data[(8 * removed.width + 8) * 4 + 3], 255)
        const split = (
          await invoke({
            operation: 'frames',
            images: background.output.frames,
            settings: { columns: 2, rows: 1, frameCount: 2, trim: false, padding: 0 },
          })
        ).result
        assert.equal(split.output.frames.length, 2)
        assert.ok(split.output.frames.every((frame) => frame.width === 16 && frame.height === 16))
        const preview = (await invoke({ operation: 'preview', images: split.output.frames })).result
        assert.deepEqual(preview.output.frames, split.output.frames)
        const transformed = (
          await invoke({
            operation: 'transform',
            images: split.output.frames,
            settings: {
              trim: false,
              align: 'none',
              padding: 0,
              transforms: [
                { index: 0, opacity: 0.5 },
                { index: 1, opacity: 1 },
              ],
            },
          })
        ).result
        const original = await pixels(split.output.frames[0].media)
        const faded = await pixels(transformed.output.frames[0].media)
        assert.deepEqual([faded.width, faded.height], [original.width, original.height])
        for (let index = 0; index < original.data.length; index += 4) {
          assert.deepEqual(
            [...faded.data.subarray(index, index + 3)],
            [...original.data.subarray(index, index + 3)],
          )
          assert.equal(faded.data[index + 3], Math.round(original.data[index + 3] * 0.5))
        }
        const edited = (
          await invoke({
            operation: 'edit',
            images: split.output.frames,
            edits: {
              frames: [
                { sourceIndex: 0, opacity: 0.25, durationMs: 240 },
                { sourceIndex: 0, durationMs: 300 },
              ],
            },
          })
        ).result
        assert.equal(edited.output.frames.length, 2)
        assert.deepEqual(
          edited.output.frames.map((frame) => frame.durationMs),
          [240, 300],
        )
        const manual = await pixels(edited.output.frames[0].media)
        for (let index = 3; index < original.data.length; index += 4)
          assert.equal(manual.data[index], Math.round(original.data[index] * 0.25))
        assert.deepEqual(
          await pixels(split.output.frames[0].media),
          original,
          'Manual edits must preserve the actual original pixels',
        )
        const publicExport = (await invoke({ operation: 'export', images: edited.output.frames }))
          .result
        const publicEvidence = await exported(publicExport, parent.id)
        const marker = `native-image-agent:${nonce}`
        const imageStart = imageFixtureRequests.length
        const generated = (
          await invoke({
            operation: 'generate',
            images: input.output.frames,
            prompt: marker,
            model: `${imageProviderId}/${imageModelId}`,
            settings: { directions: ['S'], frameCount: 2, colors: ['#ffffff'] },
          })
        ).result
        const calls = imageFixtureRequests
          .slice(imageStart)
          .filter((entry) => entry.prompt.includes(marker))
        assert.equal(
          calls.length,
          1,
          'One direction requires one real loopback Provider request, with no retry',
        )
        assert.equal(calls[0].path, '/v1/images/edits')
        assert.equal(calls[0].model, imageModelId)
        assert.equal(calls[0].imageCount, 1)
        assert.equal(calls[0].images[0].mimeType, 'image/png')
        assert.deepEqual([calls[0].images[0].width, calls[0].images[0].height], [32, 16])
        assert.equal(generated.output.frames.length, 1)
        assert.equal(generated.output.frames[0].frameCount, 2)
        assert.equal(generated.output.frames[0].direction, 'S')
        const childBefore = {
          media: (await readdir(mediaRoot)).sort(),
          exports: (await readdir(join(fixture, 'generated/image-assets'))).sort(),
          assets: (await json(`/api/assets?sessionId=${encodeURIComponent(parent.id)}`)).assets
            .map((asset) => asset.id)
            .sort(),
        }
        const childStart = fixtureRequests.length
        const spawned = await json(`/api/sessions/${parent.id}/agents`, 'POST', {
          taskName: `image-export-${nonce.slice(0, 8)}`,
          message: prompt({ operation: 'export', images: generated.output.frames }),
        })
        child = await poll(
          async () =>
            (await json(`/api/sessions/${parent.id}/agents`)).agents.find(
              (entry) => entry.id === spawned.agent.id,
            ),
          (value) => value && terminal.has(value.status),
          'Real private child image export',
          20000,
        )
        assert.equal(child.status, 'completed', JSON.stringify(child))
        // Release snapshots direct active tools at spawn. An inactive optional
        // image tool is not delegated by merely enabling it for the parent.
        assert.ok(Array.isArray(child.availableTools), JSON.stringify(child))
        assert.equal(child.availableTools.includes('image_assets'), false, JSON.stringify(child))
        const childResults = fixtureRequests.slice(childStart).flatMap((entry) => entry.toolResults)
        const denied = childResults.filter((result) =>
          /Optional tool is unavailable: image_assets/.test(result.text),
        )
        assert.equal(
          denied.length,
          1,
          `The actual child model must receive the scope rejection: ${JSON.stringify({ child, childResults })}`,
        )
        assert.ok(
          child.tools.some((tool) => tool.name === 'call_tool' && tool.status === 'error'),
          JSON.stringify({ child, childResults }),
        )
        const childAfter = {
          media: (await readdir(mediaRoot)).sort(),
          exports: (await readdir(join(fixture, 'generated/image-assets'))).sort(),
          assets: (await json(`/api/assets?sessionId=${encodeURIComponent(parent.id)}`)).assets
            .map((asset) => asset.id)
            .sort(),
        }
        assert.deepEqual(
          childAfter,
          childBefore,
          'A rejected child call must not create media, exports or archived assets',
        )
        const directoryCount = (await readdir(join(fixture, 'generated/image-assets'))).length
        const mediaCount = (await readdir(mediaRoot)).length
        await setTools(
          [...new Set([...originalApp.enabledTools, ...before.enabledTools])].filter(
            (name) => name !== 'image_assets',
          ),
        )
        await invoke({ operation: 'export', images: edited.output.frames }, true)
        assert.equal(
          (await readdir(join(fixture, 'generated/image-assets'))).length,
          directoryCount,
        )
        assert.equal((await readdir(mediaRoot)).length, mediaCount)
        assert.deepEqual(await hashes(), providerHashes)
        assert.deepEqual(unrelatedApp(await app()), unrelatedApp(originalApp))
        evidence = {
          sessionId: parent.id,
          childId: child.id,
          mode: 'workspace-write',
          actualPiGateway: true,
          relativeImportProvesCwd: true,
          agentMediaIsolated: true,
          nativePixelsVerified: true,
          providerRequests: calls.length,
          publicExport: publicEvidence,
          ordinaryChildDenied: {
            directSnapshotExcludesImageAssets: true,
            realToolErrors: denied.length,
            producedNoResources: true,
            nativeExtraGatewayExposure: child.availableTools.includes('call_tool'),
          },
          privateOwnerExportVerified: false,
          privateOwnerExportPending: 'Requires an explicit release-authorized child tool path',
          disabledProducedNoFiles: true,
          providerFilesPreserved: true,
        }
      } catch (failure) {
        failures.push(failure)
      } finally {
        try {
          await cleanup()
        } catch (failure) {
          failures.push(failure)
        }
      }
      if (failures.length)
        throw new AggregateError(
          failures,
          failures.map((error) => error.stack || String(error)).join('\n'),
        )
      return evidence
    },
  )
}
