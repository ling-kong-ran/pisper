import assert from 'node:assert/strict'
import { createHash, randomUUID } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { PNG } from 'pngjs'
import {
  parseGameAssetJob,
  parseGameAssetProject,
  parseGameAssetsCatalog,
} from '../shared/game/game-assets.mjs'
import { parseWorkflowMedia } from '../shared/workflow/workflow-inputs.mjs'

const terminal = new Set(['completed', 'failed', 'cancelled', 'interrupted'])
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex')

// This is a reference upload, not a replacement for the native model request.
export function gameAssetReferencePng(background = '#ff00ff', frameCount = 2) {
  const color = background.replace(/^#/, '')
  assert.match(color, /^[0-9a-f]{6}$/i)
  assert.ok(Number.isSafeInteger(frameCount) && frameCount >= 1 && frameCount <= 16)
  const columns = Math.ceil(Math.sqrt(frameCount))
  const rows = Math.ceil(frameCount / columns)
  const image = new PNG({ width: columns * 16, height: rows * 16 })
  const rgb = [0, 2, 4].map((offset) => Number.parseInt(color.slice(offset, offset + 2), 16))
  for (let y = 0; y < image.height; y++) {
    for (let x = 0; x < image.width; x++) {
      const frame = Math.floor(y / 16) * columns + Math.floor(x / 16)
      const foreground =
        frame < frameCount && x % 16 >= 5 && x % 16 <= 10 && y % 16 >= 3 && y % 16 <= 12
      const pixel = foreground ? [40 + frame * 7, 70, 140, 255] : [...rgb, 255]
      image.data.set(pixel, (y * image.width + x) * 4)
    }
  }
  return PNG.sync.write(image)
}

function collectedFailure(failures) {
  if (failures.length === 1) return failures[0]
  return new AggregateError(
    failures,
    failures.map((error) => error.stack || String(error)).join('\n'),
  )
}

// Root owns the server, synthetic provider, browser and restart. No runtime is
// booted here, and no configuration or provider credentials are read from disk.
export async function checkGameAssetParity({
  check,
  json,
  request,
  workspace,
  agent,
  imageProviderId,
  imageModelId,
  imageBaseUrl,
  imageFixtureRequests,
  heldImageRequests,
  waitForHeldImage,
  delay,
  retainForRestart = true,
}) {
  assert.equal(
    typeof agent,
    'string',
    'Pass the isolated agent directory for durable storage proof',
  )
  assert.equal(typeof workspace, 'string', 'Use the root synthetic workspace')
  const prefix = '/api/game-assets'
  const owned = new Map()
  let reference
  let retained
  const model = { provider: imageProviderId, model: imageModelId }
  const imageCalls = () => {
    assert.ok(
      Array.isArray(imageFixtureRequests),
      'Root must expose actual local image HTTP requests',
    )
    return imageFixtureRequests.filter((entry) => entry.model === imageModelId)
  }

  function response(path, method, body, timeout = 10000) {
    return request(path, {
      method,
      ...(body === undefined
        ? {}
        : {
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
          }),
      signal: AbortSignal.timeout(timeout),
    })
  }

  async function statusJson(path, method, body, status, timeout) {
    const result = await response(path, method, body, timeout)
    const value = await result.json()
    assert.equal(result.status, status, `${method} ${path}: ${JSON.stringify(value)}`)
    return value
  }

  async function error(path, method, body, status, code, publicMessage = code) {
    const value = await statusJson(path, method, body, status)
    assert.equal(value.code, code)
    assert.equal(
      value.error,
      publicMessage,
      'Public errors must match the safe endpoint contract without upstream or storage details',
    )
    return value
  }

  async function poll(read, predicate, label, timeout = 20000) {
    const deadline = Date.now() + timeout
    let latest
    while (Date.now() < deadline) {
      latest = await read(Math.max(1, Math.min(5000, deadline - Date.now())))
      if (predicate(latest)) return latest
      await delay(50)
    }
    assert.fail(`${label} did not settle within ${timeout}ms: ${JSON.stringify(latest)}`)
  }

  async function catalog() {
    const value = await json(prefix)
    const parsed = parseGameAssetsCatalog({ projects: value.projects, jobs: value.jobs })
    assert.ok(Array.isArray(value.models))
    assert.ok(Array.isArray(value.engines))
    for (const entry of value.models) {
      assert.deepEqual(Object.keys(entry).sort(), ['id', 'name', 'providerId', 'providerName'])
      assert.ok(entry.id.startsWith(entry.providerId + '/'))
    }
    return { ...parsed, models: value.models, engines: value.engines }
  }

  async function png(media) {
    parseWorkflowMedia(media)
    const result = await request(`${prefix}/media/${media.id}/content`)
    assert.equal(result.status, 200)
    assert.equal(result.headers.get('content-type'), media.mimeType)
    const bytes = Buffer.from(await result.arrayBuffer())
    assert.equal(bytes.length, media.size)
    const image = PNG.sync.read(bytes)
    return { bytes, image, hash: digest(bytes) }
  }

  async function atlas(job) {
    const output = job.output
    assert.ok(output.atlas, 'A completed pipeline must persist the real PNG atlas')
    assert.equal(output.atlas.frames.length, output.frames.length)
    const result = await png(output.atlas.media)
    assert.equal(result.image.width, output.atlas.width)
    assert.equal(result.image.height, output.atlas.height)
    for (const frame of output.atlas.frames) {
      assert.ok(frame.x >= 0 && frame.y >= 0)
      assert.ok(frame.x + frame.width <= result.image.width)
      assert.ok(frame.y + frame.height <= result.image.height)
    }
    return result
  }

  function input(name, overrides = {}) {
    return {
      name: `native-game-smoke-${name}`,
      prompt: 'Keep the supplied character style.',
      reference,
      originalReference: reference,
      frameCount: 2,
      directions: ['S', 'N'],
      model,
      actions: [
        { id: 'idle', name: 'Idle', prompt: 'Breathe', enabled: true },
        { id: 'walk', name: 'Walk', prompt: 'Move forward', enabled: true },
      ],
      ...overrides,
    }
  }

  async function create(value) {
    const project = parseGameAssetProject(
      await statusJson(`${prefix}/projects`, 'POST', value, 201),
    )
    owned.set(project.id, new Set())
    return project
  }

  async function start(project) {
    const value = await statusJson(`${prefix}/projects/${project.id}/run`, 'POST', {}, 202)
    assert.deepEqual(Object.keys(value), ['job'])
    const job = parseGameAssetJob(value.job)
    assert.equal(job.projectId, project.id)
    owned.get(project.id).add(job.id)
    return job
  }

  function settled(job, timeout = 20000) {
    return poll(
      async (remaining) =>
        parseGameAssetJob(await json(`${prefix}/jobs/${job.id}`, 'GET', undefined, remaining)),
      (current) => terminal.has(current.status),
      `Game job ${job.id}`,
      timeout,
    )
  }

  async function remove(projectId) {
    const failures = []
    for (const jobId of owned.get(projectId) || []) {
      try {
        const result = await response(`${prefix}/jobs/${jobId}/stop`, 'POST', {}, 10000)
        const value = await result.json()
        assert.ok([200, 404].includes(result.status), `Stop owned job: ${JSON.stringify(value)}`)
        if (result.status === 200) assert.ok(terminal.has(parseGameAssetJob(value.job).status))
      } catch (failure) {
        failures.push(failure)
      }
    }
    try {
      const result = await response(`${prefix}/projects/${projectId}`, 'DELETE')
      const value = await result.json()
      assert.ok(
        [200, 404].includes(result.status),
        `Remove owned project: ${JSON.stringify(value)}`,
      )
      if (result.status === 200) assert.equal(value.deleted, true)
      owned.delete(projectId)
    } catch (failure) {
      failures.push(failure)
    }
    if (failures.length) throw collectedFailure(failures)
  }

  async function withProject(value, test) {
    let project
    let keep = false
    let evidence
    const failures = []
    try {
      project = await create(value)
      evidence = await test(project, () => {
        keep = true
      })
    } catch (failure) {
      failures.push(failure)
    }
    if (project && !keep) {
      try {
        await remove(project.id)
      } catch (failure) {
        failures.push(failure)
      }
    }
    if (failures.length) throw collectedFailure(failures)
    return evidence
  }

  await check('Game visual provider is configured through the native provider API', async () => {
    assert.equal(typeof imageProviderId, 'string')
    assert.equal(typeof imageModelId, 'string')
    const endpoint = new URL(imageBaseUrl)
    assert.ok(
      ['127.0.0.1', 'localhost', '[::1]'].includes(endpoint.hostname),
      'Use only the root loopback image fixture',
    )
    const previous = await json('/api/config')
    const existing = previous.providers.find((entry) => entry.id === imageProviderId)
    const value = existing
      ? previous
      : await statusJson(
          '/api/providers',
          'POST',
          {
            id: imageProviderId,
            name: 'Native Game Image Fixture',
            providerType: 'visual',
            api: 'openai-completions',
            baseUrl: imageBaseUrl,
            apiKey: 'synthetic-rust-test-key',
            model: imageModelId,
            modelKind: 'image',
            enabled: true,
          },
          201,
        )
    const provider = value.providers.find((entry) => entry.id === imageProviderId)
    assert.equal(provider.configured, true)
    assert.equal(provider.baseUrl, imageBaseUrl)
    assert.ok(
      (await catalog()).models.some((entry) => entry.id === `${imageProviderId}/${imageModelId}`),
    )
    assert.equal(
      value.defaultProvider || value.provider,
      previous.defaultProvider || previous.provider,
    )
    assert.equal(value.defaultModel || value.model, previous.defaultModel || previous.model)
    return {
      providerId: imageProviderId,
      imageModelId,
      localFixture: true,
      chatDefaultPreserved: true,
    }
  })

  await check('Game asset catalog and independent draft CRUD obey the release schema', () =>
    withProject({ name: 'native-game-smoke-empty-draft' }, async (project) => {
      assert.equal(project.reference, null)
      assert.deepEqual(project.actions, [])
      const value = await catalog()
      assert.ok(value.projects.some((entry) => entry.id === project.id))
      assert.ok(value.engines.some((entry) => entry.id === 'background'))
      assert.ok(value.engines.some((entry) => entry.id === 'inpaint'))
      const changed = parseGameAssetProject(
        await statusJson(
          `${prefix}/projects/${project.id}`,
          'PATCH',
          {
            name: 'native-game-smoke-renamed-draft',
            frameCount: 2,
            directions: ['S'],
          },
          200,
        ),
      )
      assert.equal(changed.id, project.id)
      assert.equal(changed.createdAt, project.createdAt)
      assert.equal(changed.name, 'native-game-smoke-renamed-draft')
      await error(
        `${prefix}/projects/${project.id}/run`,
        'POST',
        {},
        400,
        'game_assets_source_required',
      )
      await error(
        `${prefix}/projects`,
        'POST',
        { name: 'bad', nodes: [] },
        400,
        'game_assets_invalid',
      )
      await error(`${prefix}/projects/not-a-uuid`, 'DELETE', undefined, 400, 'game_assets_invalid')
      return { projectId: project.id, strictSchema: true, emptyDraftSaved: true }
    }),
  )

  await check(
    'Game media raw upload, native color processing and workflow media isolation',
    async () => {
      const bytes = gameAssetReferencePng()
      const result = await request(`${prefix}/media?name=native-game-reference.png`, {
        method: 'POST',
        headers: { 'Content-Type': 'image/png' },
        body: bytes,
      })
      assert.equal(result.status, 201)
      reference = parseWorkflowMedia(await result.json())
      assert.equal(reference.name, 'native-game-reference.png')
      assert.deepEqual((await png(reference)).bytes, bytes)
      await error(
        `/api/workflow-media/${reference.id}/content`,
        'GET',
        undefined,
        404,
        'workflow_media_missing',
        '工作流素材上传或读取失败。',
      )
      const before = imageFixtureRequests?.length
      const processed = parseWorkflowMedia(
        await statusJson(
          `${prefix}/process`,
          'POST',
          {
            reference,
            operation: 'background',
            image: { method: 'color', colors: ['#ff00ff'] },
          },
          200,
        ),
      )
      assert.notEqual(processed.id, reference.id)
      const image = (await png(processed)).image
      assert.equal(image.data[3], 0)
      assert.deepEqual(
        Array.from(image.data.subarray((8 * image.width + 8) * 4, (8 * image.width + 8) * 4 + 4)),
        [40, 70, 140, 255],
      )
      const repeated = parseWorkflowMedia(
        await statusJson(
          `${prefix}/process`,
          'POST',
          {
            reference,
            operation: 'background',
            image: { method: 'color', colors: ['#ff00ff'] },
          },
          200,
        ),
      )
      assert.deepEqual((await png(repeated)).image.data, image.data)
      if (before !== undefined) assert.equal(imageFixtureRequests.length, before)
      const invalid = await request(`${prefix}/media`, {
        method: 'POST',
        headers: { 'Content-Type': 'text/plain' },
        body: 'not an image',
      })
      assert.equal(invalid.status, 415)
      assert.equal((await invalid.json()).code, 'game_assets_media_invalid')
      return {
        mediaId: reference.id,
        realPngBytes: bytes.length,
        colorProcessingLocal: true,
        independentMedia: true,
      }
    },
  )

  await check(
    'Game generation uses native provider HTTP then edits and exports from immutable originals',
    async () => {
      assert.ok(reference, 'Reference upload must pass before actual generation')
      assert.equal(typeof imageProviderId, 'string', 'Pass the configured local visual provider')
      assert.equal(typeof imageModelId, 'string', 'Pass its image model ID')
      const candidates = (await catalog()).models
      assert.ok(candidates.some((entry) => entry.id === `${imageProviderId}/${imageModelId}`))
      const promptMarker = `native-game-persist:${randomUUID()}`
      return withProject(input('generated', { prompt: promptMarker }), async (project, keep) => {
        const before = imageCalls().length
        const initial = await start(project)
        const completed = await settled(initial)
        assert.equal(completed.status, 'completed', JSON.stringify(completed))
        assert.equal(completed.completed, 2)
        assert.equal(completed.total, 2)
        assert.equal(completed.output.frames.length, 8)
        assert.deepEqual(completed.originalOutput, completed.output)
        assert.equal(completed.revision, 0)
        assert.equal(
          imageCalls().length - before,
          4,
          'Two actions and two directions require four real paid-request fixtures',
        )
        const generated = imageCalls().slice(before)
        for (const call of generated) {
          assert.equal(
            call.path,
            '/v1/images/edits',
            'The reference image must reach the native multipart path',
          )
          assert.match(call.prompt, /2-frame continuous/)
          assert.ok(call.imageCount >= 1, 'The actual provider request carries the source PNG')
        }
        const originalAtlas = await atlas(completed)
        assert.ok(originalAtlas.image.data.some((alpha, index) => index % 4 === 3 && alpha === 0))
        assert.ok(originalAtlas.image.data.some((alpha, index) => index % 4 === 3 && alpha === 255))
        const source = await png(completed.originalOutput.frames[6].media)
        const firstBody = {
          frames: [
            { sourceIndex: 6, opacity: 0.5, durationMs: 240 },
            { sourceIndex: 1, x: 1, rotation: 15, scale: 0.8 },
            {
              sourceIndex: 1,
              eraseStrokes: [
                {
                  radius: 0.1,
                  points: [
                    { x: 0.25, y: 0.25 },
                    { x: 0.75, y: 0.75 },
                  ],
                },
              ],
            },
          ],
        }
        const edited = parseGameAssetJob(
          await statusJson(`${prefix}/jobs/${completed.id}/frames`, 'POST', firstBody, 200, 20000),
        )
        assert.equal(
          edited.id,
          completed.id,
          'Frames returns the job directly and waits for durable export',
        )
        assert.equal(edited.revision, 1)
        assert.equal(edited.output.frames.length, 3)
        assert.deepEqual(edited.originalOutput, completed.originalOutput)
        assert.equal(edited.output.frames[0].durationMs, 240)
        assert.equal(
          edited.output.frames[0].direction,
          completed.originalOutput.frames[6].direction,
        )
        const faded = (await png(edited.output.frames[0].media)).image
        assert.equal(faded.width, source.image.width)
        assert.equal(faded.height, source.image.height)
        for (let offset = 0; offset < faded.data.length; offset += 4) {
          assert.deepEqual(
            faded.data.subarray(offset, offset + 3),
            source.image.data.subarray(offset, offset + 3),
          )
          assert.equal(faded.data[offset + 3], Math.round(source.image.data[offset + 3] * 0.5))
        }
        await atlas(edited)
        const second = parseGameAssetJob(
          await statusJson(
            `${prefix}/jobs/${completed.id}/frames`,
            'POST',
            {
              frames: [{ sourceIndex: 7, durationMs: 300 }, { sourceIndex: 6 }],
            },
            200,
            20000,
          ),
        )
        assert.equal(second.revision, 2)
        assert.equal(second.output.frames.length, 2)
        assert.deepEqual(second.originalOutput, completed.originalOutput)
        assert.deepEqual(
          (await png(second.output.frames[0].media)).image.data,
          (await png(completed.originalOutput.frames[7].media)).image.data,
        )
        assert.deepEqual((await png(second.output.frames[1].media)).image.data, source.image.data)
        assert.equal(second.output.frames[0].durationMs, 300)
        const exported = await atlas(second)
        assert.equal(
          imageCalls().length - before,
          4,
          'Manual replay and atlas exports must never call the image provider',
        )
        for (const [body, code] of [
          [{ frames: [] }, 'workflow_image_invalid_edits'],
          [{ frames: [{ sourceIndex: 8 }] }, 'game_assets_invalid'],
          [{ frames: [{ sourceIndex: 0, opacity: 2 }] }, 'workflow_image_invalid_edits'],
        ]) {
          await error(`${prefix}/jobs/${completed.id}/frames`, 'POST', body, 400, code)
          assert.deepEqual(
            await json(`${prefix}/jobs/${completed.id}`),
            second,
            'Invalid edits must leave the durable revision unchanged',
          )
        }
        const latest = await catalog()
        assert.deepEqual(
          latest.jobs.filter((entry) => entry.projectId === project.id),
          [second],
        )
        const stored = JSON.parse(
          await readFile(join(resolve(agent), 'game-assets', 'game-assets.json'), 'utf8'),
        )
        assert.equal(stored.version, 1)
        assert.deepEqual(
          stored.jobs.find((entry) => entry.id === second.id),
          second,
        )
        assert.deepEqual(
          stored.projects.find((entry) => entry.id === project.id),
          project,
        )
        for (const forbidden of ['workflowId', 'nodeId', 'apiKey', 'baseUrl']) {
          assert.ok(!JSON.stringify(stored).includes(`"${forbidden}"`))
        }
        retained = {
          project,
          job: second,
          atlasHash: exported.hash,
          promptMarker,
          imageRequestCount: imageCalls().filter((call) => call.prompt.includes(promptMarker))
            .length,
        }
        if (retainForRestart) keep()
        return {
          projectId: project.id,
          jobId: second.id,
          frames: 8,
          revision: 2,
          atlasHash: exported.hash,
          nativeProviderCalls: generated.length,
          immutableReplay: true,
        }
      })
    },
  )

  await check(
    'Game jobs enforce two active projects and stop their actual image HTTP connections',
    async () => {
      assert.equal(
        typeof waitForHeldImage,
        'function',
        'Root must expose actual held image connection state',
      )
      assert.ok(heldImageRequests instanceof Map)
      const projects = []
      const labels = ['game-first', 'game-second', 'game-third']
      const failures = []
      let evidence
      try {
        for (const label of labels)
          projects.push(
            await create(
              input(label, {
                prompt: `native-game-hold:${label}`,
                directions: ['S'],
                actions: [{ id: 'idle', name: 'Idle', prompt: '', enabled: true }],
              }),
            ),
          )
        const first = await start(projects[0])
        const second = await start(projects[1])
        await Promise.all(labels.slice(0, 2).map((label) => waitForHeldImage(label, true, 10000)))
        await error(`${prefix}/projects/${projects[0].id}/run`, 'POST', {}, 409, 'game_assets_busy')
        await error(
          `${prefix}/projects/${projects[0].id}`,
          'DELETE',
          undefined,
          409,
          'game_assets_busy',
        )
        await error(`${prefix}/projects/${projects[2].id}/run`, 'POST', {}, 409, 'game_assets_busy')
        const stopped = parseGameAssetJob(
          (await statusJson(`${prefix}/jobs/${first.id}/stop`, 'POST', {}, 200)).job,
        )
        assert.equal(stopped.status, 'cancelled')
        assert.equal(stopped.error, 'game_assets_cancelled')
        await waitForHeldImage(labels[0], false, 5000)
        assert.equal(
          heldImageRequests.has(labels[1]),
          true,
          'Cancelling one project must preserve unrelated provider work',
        )
        const third = await start(projects[2])
        await waitForHeldImage(labels[2], true, 10000)
        for (const job of [second, third]) {
          const stop = await statusJson(`${prefix}/jobs/${job.id}/stop`, 'POST', {}, 200)
          assert.equal(parseGameAssetJob(stop.job).status, 'cancelled')
        }
        await Promise.all(labels.slice(1).map((label) => waitForHeldImage(label, false, 5000)))
        evidence = {
          cancelledJob: first.id,
          admissionReleased: true,
          unrelatedCallSurvived: true,
          maxActiveProjects: 2,
        }
      } catch (failure) {
        failures.push(failure)
      }
      for (const project of projects) {
        try {
          await remove(project.id)
        } catch (failure) {
          failures.push(failure)
        }
      }
      for (const label of labels) heldImageRequests.get(label)?.()
      if (failures.length) throw collectedFailure(failures)
      return evidence
    },
  )

  await check('Game paid generation failure retains completed direction pixels without retry', () =>
    withProject(
      input('partial', {
        prompt: 'native-game-fail-second-direction',
        actions: [{ id: 'idle', name: 'Idle', prompt: '', enabled: true }],
      }),
      async (project) => {
        const before = imageCalls().length
        const failed = await settled(await start(project))
        assert.equal(failed.status, 'failed', JSON.stringify(failed))
        assert.equal(failed.error, 'workflow_image_generation_failed')
        assert.equal(failed.completed, 0)
        assert.equal(
          failed.output.frames.length,
          1,
          'The first successful paid sprite sheet must remain available',
        )
        assert.equal(failed.output.frames[0].direction, 'S')
        assert.deepEqual(failed.originalOutput, failed.output)
        assert.equal(failed.output.atlas, undefined)
        await png(failed.output.frames[0].media)
        assert.equal(
          imageCalls().length - before,
          2,
          'A failed paid direction must not be silently retried',
        )
        return {
          jobId: failed.id,
          preservedSheets: failed.output.frames.length,
          paidRequests: 2,
          retried: false,
        }
      },
    ),
  )

  async function cleanup() {
    const failures = []
    for (const id of [...owned.keys()]) {
      try {
        await remove(id)
      } catch (failure) {
        failures.push(failure)
      }
    }
    if (failures.length) throw collectedFailure(failures)
  }

  async function verifyAfterRestart() {
    assert.ok(
      retained && retainForRestart,
      'A successful completed fixture must be retained for real restart',
    )
    const failures = []
    let evidence
    try {
      const value = await catalog()
      assert.deepEqual(
        value.projects.find((entry) => entry.id === retained.project.id),
        retained.project,
      )
      assert.deepEqual(
        value.jobs.find((entry) => entry.id === retained.job.id),
        retained.job,
      )
      assert.deepEqual(await json(`${prefix}/jobs/${retained.job.id}`), retained.job)
      assert.equal((await atlas(retained.job)).hash, retained.atlasHash)
      assert.equal(
        imageCalls().filter((call) => call.prompt.includes(retained.promptMarker)).length,
        retained.imageRequestCount,
        'Restart must not replay completed paid generation',
      )
      evidence = {
        jobId: retained.job.id,
        revision: retained.job.revision,
        atlasHash: retained.atlasHash,
        paidGenerationReplayed: false,
      }
    } catch (failure) {
      failures.push(failure)
    }
    try {
      await cleanup()
    } catch (failure) {
      failures.push(failure)
    }
    if (failures.length) throw collectedFailure(failures)
    return evidence
  }

  verifyAfterRestart.cleanup = cleanup
  verifyAfterRestart.restartFixture = retained
  return verifyAfterRestart
}

// The real, version-pinned external component is imported into the isolated test
// profile through its browser UI. All subsequent operations use its host bridge.
export async function checkGameAssetUiParity({
  check,
  page,
  base,
  output,
  json,
  request,
  workspace,
  agent,
  workbenchZip,
  imageProviderId,
  imageModelId,
  delay,
}) {
  assert.equal(typeof workspace, 'string')
  assert.equal(typeof agent, 'string')
  const projects = new Set()
  const jobs = new Set()
  const workflowWrites = []
  const pageErrors = []
  const apiTrace = []
  const inspectRequest = (value) => {
    const path = new URL(value.url()).pathname
    if (path.startsWith('/api/game-assets'))
      apiTrace.push({ type: 'request', path, method: value.method() })
    if (/^\/api\/workflow(?:s|-runs|-media)(?:\/|$)/.test(path) && value.method() !== 'GET') {
      workflowWrites.push({ path, method: value.method() })
    }
  }
  const inspectError = (error) => pageErrors.push(error.message)
  const inspectResponse = (value) => {
    const path = new URL(value.url()).pathname
    if (path.startsWith('/api/game-assets'))
      apiTrace.push({ type: 'response', path, status: value.status() })
  }
  const inspectFailed = (value) => {
    const path = new URL(value.url()).pathname
    if (path.startsWith('/api/game-assets'))
      apiTrace.push({ type: 'failed', path, error: value.failure()?.errorText })
  }
  await page.addInitScript(() => {
    window.__pisperGameBridgeTrace = []
    window.addEventListener('message', (event) => {
      const data = event.data
      if (!data || data.pisperBridge !== 1) return
      let entry
      if (typeof data.method === 'string' && data.method.startsWith('gameAssets.')) {
        entry = {
          type: 'request',
          id: data.id,
          method: data.method,
          operation: data.params?.operation,
          imageMethod: data.params?.image?.method,
        }
      } else if (typeof data.ok === 'boolean') {
        entry = {
          type: 'reply',
          id: data.id,
          ok: data.ok,
          ...(data.ok ? {} : { error: String(data.error).slice(0, 300) }),
        }
      }
      if (entry) {
        window.__pisperGameBridgeTrace.push(entry)
        if (window.__pisperGameBridgeTrace.length > 512) window.__pisperGameBridgeTrace.shift()
      }
    })
  })

  function waitResponse(path, method = 'POST', timeout = 20000) {
    return page.waitForResponse(
      (value) => {
        const actual = new URL(value.url()).pathname
        return (
          (typeof path === 'string' ? actual === path : path.test(actual)) &&
          value.request().method() === method
        )
      },
      { timeout },
    )
  }

  async function png(media) {
    const response = await request(`/api/game-assets/media/${media.id}/content`)
    assert.equal(response.status, 200)
    return PNG.sync.read(Buffer.from(await response.arrayBuffer()))
  }

  async function settle(id) {
    const deadline = Date.now() + 20000
    let job
    while (Date.now() < deadline) {
      job = parseGameAssetJob(await json(`/api/game-assets/jobs/${id}`))
      if (terminal.has(job.status)) return job
      await delay(50)
    }
    assert.fail(`Actual browser game job did not settle: ${JSON.stringify(job)}`)
  }

  await check(
    'Browser Game Asset Workbench imports, uploads, generates, edits, downloads and reopens through its real bridge',
    async () => {
      const failures = []
      let evidence
      page.on('request', inspectRequest)
      page.on('pageerror', inspectError)
      page.on('response', inspectResponse)
      page.on('requestfailed', inspectFailed)
      try {
        assert.ok(workbenchZip?.buffer, 'Pass the complete pinned real workbench ZIP')
        assert.equal(
          digest(workbenchZip.buffer),
          'dfb6f1372ba9f6e4f9ff0137399f4ede74c3d15fc857ee0917a29ae80c162730',
        )
        await page.goto(base + '/#/config/interface?view=widgets')
        const importResponse = waitResponse('/api/custom-ui/import')
        const chooser = page.waitForEvent('filechooser')
        await page.getByRole('button', { name: /导入 ZIP|Import ZIP/i }).click()
        await (await chooser).setFiles(workbenchZip)
        const imported = await importResponse
        assert.equal(imported.status(), 201)
        assert.equal((await imported.json()).id, 'pisper-game-asset-workbench')
        await page.getByRole('dialog', { name: /已导入|Imported/i }).waitFor()
        await page.getByRole('button', { name: /稍后|Later/i, exact: true }).click()
        await page.goto(base + '/#/chat')
        await page.reload()
        await page.getByRole('button', { name: /更多工具|More tools/i, exact: true }).click()
        await page.getByRole('menuitem', { name: 'Game Asset Workbench', exact: true }).click()
        await page.waitForURL(/#\/tools\/components\/pisper-game-asset-workbench$/)
        const widget = page.frameLocator('iframe[title="Game Asset Workbench"]')
        await widget.locator('#save:not(:disabled)').waitFor()
        assert.equal(
          await page.locator('iframe[title="Game Asset Workbench"]').getAttribute('sandbox'),
          'allow-scripts',
        )
        // Native keyboard activation reaches the button without iframe pointer hit testing.
        await widget.locator('#new:not(:disabled)').press('Enter')
        // Activation still must produce the component's real resulting draft state.
        await widget.locator('#project:not(:disabled) option[value=""]:checked').waitFor({
          state: 'attached',
        })
        assert.equal(await widget.locator('#project').inputValue(), '')
        assert.match(
          await widget.locator('#name').inputValue(),
          /^(Game Asset Workbench|游戏素材工作台)$/,
        )
        await widget.locator('#name').fill('native-game-ui-empty-draft')
        const draftResponse = waitResponse('/api/game-assets/projects')
        await widget.locator('#save').click()
        const draftResult = await draftResponse
        assert.equal(draftResult.status(), 201)
        const draft = parseGameAssetProject(await draftResult.json())
        projects.add(draft.id)
        assert.equal(draft.name, 'native-game-ui-empty-draft')
        assert.equal(draft.reference, null)
        // 保存先选中 draft，再 await refresh；等保存与刷新全部解锁后，再新建。
        await widget.locator(`#project option[value="${draft.id}"]:checked`).waitFor({
          state: 'attached',
        })
        await widget.locator('#save:not(:disabled)').waitFor()
        await widget.locator('#new:not(:disabled)').press('Enter')
        await widget.locator('#project:not(:disabled) option[value=""]:checked').waitFor({
          state: 'attached',
        })
        assert.equal(await widget.locator('#project').inputValue(), '')
        assert.match(
          await widget.locator('#name').inputValue(),
          /^(Game Asset Workbench|游戏素材工作台)$/,
        )
        await widget.locator('#name').fill('native-game-ui-character')
        await widget.locator('#prompt').fill('Keep the supplied character style.')
        await widget.locator('#model').selectOption(`${imageProviderId}/${imageModelId}`)
        await widget.locator('#frames').fill('2')
        const directions = widget.locator('#directions input[type="checkbox"]')
        for (let index = 1; index < (await directions.count()); index++)
          await directions.nth(index).uncheck()
        const actions = widget.locator('#actions input[type="checkbox"]')
        for (let index = 1; index < (await actions.count()); index++)
          await actions.nth(index).uncheck()
        const uploadResponse = waitResponse('/api/game-assets/media')
        await widget.locator('#upload').setInputFiles({
          name: 'character.png',
          mimeType: 'image/png',
          buffer: gameAssetReferencePng('#ffffff'),
        })
        const uploadedResult = await uploadResponse
        assert.equal(uploadedResult.status(), 201)
        const uploaded = parseWorkflowMedia(await uploadedResult.json())
        await widget.locator('#source-image').waitFor()
        await widget.locator('#background-mode').selectOption('color')
        const processResponse = waitResponse('/api/game-assets/process')
        await widget.locator('#background:not(:disabled)').click()
        const processedResult = await processResponse
        assert.equal(processedResult.status(), 200)
        const processed = parseWorkflowMedia(await processedResult.json())
        assert.equal(processedResult.request().postDataJSON().reference.id, uploaded.id)
        const pixels = await png(processed)
        assert.equal(pixels.data[3], 0)
        assert.equal(pixels.data[(8 * pixels.width + 8) * 4 + 3], 255)
        // The process HTTP response arrives before the component consumes its bridge reply.
        await widget.locator('#run:not(:disabled)').waitFor()
        assert.equal(await widget.locator('#project').inputValue(), '')
        const savedResponse = waitResponse('/api/game-assets/projects')
        const startResponse = waitResponse(/\/api\/game-assets\/projects\/[^/]+\/run$/)
        await widget.locator('#run:not(:disabled)').click()
        const savedResult = await savedResponse
        assert.equal(savedResult.status(), 201)
        const saved = parseGameAssetProject(await savedResult.json())
        projects.add(saved.id)
        assert.notEqual(saved.id, draft.id)
        assert.equal(saved.name, 'native-game-ui-character')
        assert.deepEqual(saved.directions, ['S'])
        assert.equal(saved.actions.filter((action) => action.enabled).length, 1)
        const startedResult = await startResponse
        assert.equal(startedResult.status(), 202)
        const started = parseGameAssetJob((await startedResult.json()).job)
        jobs.add(started.id)
        const completed = await settle(started.id)
        assert.equal(completed.status, 'completed', JSON.stringify(completed))
        assert.equal(completed.output.frames.length, 2)
        await widget.locator('#export-png:not(:disabled)').waitFor()
        await widget.locator('#play').click()
        await widget.locator('#play').click()
        const current = Number((await widget.locator('#counter').textContent()).split(' / ')[0])
        await widget.locator('#next').click()
        assert.equal(await widget.locator('#counter').textContent(), `${(current % 2) + 1} / 2`)
        const editor = widget.locator('#frame-editor')
        await editor.locator('summary').click()
        const original = await png(completed.originalOutput.frames[0].media)
        await editor.locator('#edit-opacity').fill('0.5')
        await editor.locator('#edit-opacity').blur()
        await editor.locator('#edit-durationMs').fill('240')
        await editor.locator('#edit-durationMs').blur()
        await editor.locator('[data-e="duplicate"]').click()
        assert.equal(await editor.locator('#edit-info').textContent(), '2 / 3')
        await editor.locator('[data-e="later"]').click()
        assert.equal(await editor.locator('#edit-info').textContent(), '3 / 3')
        await editor.locator('[data-e="remove"]').click()
        assert.equal(await editor.locator('#edit-info').textContent(), '2 / 2')
        await editor.locator('[data-e="undo"]').click()
        assert.equal(await editor.locator('#edit-info').textContent(), '3 / 3')
        await editor.locator('[data-e="redo"]').click()
        assert.equal(await editor.locator('#edit-info').textContent(), '2 / 2')
        await editor.locator('#edit-strip .edit-thumb').first().click()
        const editResponse = waitResponse(/\/api\/game-assets\/jobs\/[^/]+\/frames$/)
        await editor.locator('[data-e="apply"]').click()
        const editedResult = await editResponse
        assert.equal(editedResult.status(), 200)
        const edited = parseGameAssetJob(await editedResult.json())
        assert.equal(edited.revision, 1)
        assert.equal(edited.edits.frames[0].opacity, 0.5)
        assert.equal(edited.edits.frames[0].durationMs, 240)
        assert.deepEqual(edited.originalOutput, completed.originalOutput)
        const faded = await png(edited.output.frames[0].media)
        for (let index = 3; index < faded.data.length; index += 4) {
          assert.equal(faded.data[index], Math.round(original.data[index] * 0.5))
        }
        await widget.locator('#export-png:not(:disabled)').waitFor()
        const downloads = []
        for (const format of ['png', 'json']) {
          const downloadEvent = page.waitForEvent('download')
          await widget.locator(`#export-${format}`).click()
          const download = await downloadEvent
          const path = await download.path()
          assert.ok(path, 'The real host bridge must provide a downloadable local artifact')
          const bytes = await readFile(path)
          if (format === 'png') {
            const image = PNG.sync.read(bytes)
            assert.equal(image.width, edited.output.atlas.width)
            assert.equal(image.height, edited.output.atlas.height)
          } else assert.deepEqual(JSON.parse(bytes.toString('utf8')), edited.output.atlas)
          downloads.push({ format, bytes: bytes.length, sha256: digest(bytes) })
        }
        await page.screenshot({
          path: join(output, 'rust-game-workbench-desktop.png'),
          fullPage: true,
        })
        await page.reload()
        await widget.locator('#project').selectOption(saved.id)
        await widget.locator('#export-png:not(:disabled)').waitFor()
        await editor.locator('summary').click()
        assert.equal(await editor.locator('#edit-opacity').inputValue(), '0.5')
        assert.equal(await editor.locator('#edit-durationMs').inputValue(), '240')
        await editor.locator('[data-e="reset"]').click()
        assert.equal(await editor.locator('#edit-opacity').inputValue(), '1')
        await editor.locator('[data-e="discard"]').click()
        assert.equal(await editor.locator('#edit-opacity').inputValue(), '0.5')
        await page.setViewportSize({ width: 390, height: 844 })
        // The phone shell remounts the component; reopen the saved project and editor.
        await page.locator('[data-mobile-shell="phone"]').waitFor()
        await widget.locator('#save:not(:disabled)').waitFor()
        await widget.locator('#project').selectOption(saved.id)
        await widget.locator('#export-png:not(:disabled)').waitFor()
        if ((await editor.locator('details').getAttribute('open')) === null) {
          await editor.locator('summary').click()
        }
        await editor.locator('#edit-opacity').waitFor()
        assert.equal(await editor.locator('#edit-opacity').inputValue(), '0.5')
        assert.equal(await editor.locator('#edit-durationMs').inputValue(), '240')
        const reopened = parseGameAssetJob(await json(`/api/game-assets/jobs/${edited.id}`))
        assert.equal(reopened.revision, 1)
        assert.equal(reopened.edits.frames[0].opacity, 0.5)
        assert.equal(reopened.edits.frames[0].durationMs, 240)
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
        assert.ok(
          await widget.locator('html').evaluate((html) => html.scrollWidth <= innerWidth + 1),
        )
        await editor.locator('#edit-opacity').fill('0.6')
        await editor.locator('#edit-opacity').blur()
        await editor.locator('[data-e="apply"]').scrollIntoViewIfNeeded()
        await editor.locator('[data-e="apply"]').click({ trial: true })
        await page.screenshot({
          path: join(output, 'rust-game-workbench-mobile.png'),
          fullPage: true,
        })
        await editor.locator('[data-e="discard"]').click()
        assert.deepEqual(pageErrors, [])
        assert.deepEqual(
          workflowWrites,
          [],
          'The independent workbench must not create workflow records',
        )
        evidence = {
          componentId: 'pisper-game-asset-workbench',
          jobId: edited.id,
          revision: 1,
          realBridge: true,
          frameEditPixelsVerified: true,
          reloadPreserved: true,
          downloads,
          mobileWidth: 390,
        }
      } catch (failure) {
        failures.push(failure)
        try {
          const trace = await page.evaluate(() => window.__pisperGameBridgeTrace || [])
          const component = await page
            .frameLocator('iframe[title="Game Asset Workbench"]')
            .locator('body')
            .evaluate(() => ({
              bridge: window.__pisperGameBridgeTrace || [],
              message: document.querySelector('#message')?.textContent,
              backgroundMode: document.querySelector('#background-mode')?.value,
              backgroundDisabled: document.querySelector('#background')?.disabled,
              sourceImageVisible: Boolean(
                document.querySelector('#source-image')?.getAttribute('src'),
              ),
              projectValue: document.querySelector('#project')?.value,
              projectName: document.querySelector('#name')?.value,
              newDisabled: document.querySelector('#new')?.disabled,
              saveDisabled: document.querySelector('#save')?.disabled,
              runDisabled: document.querySelector('#run')?.disabled,
              frameEditorMessage: document.querySelector('#edit-status')?.textContent,
            }))
          await writeFile(
            join(output, 'rust-game-workbench-failure.json'),
            JSON.stringify({ apiTrace, trace, component, pageErrors }, null, 2),
          )
          await page.screenshot({
            path: join(output, 'rust-game-workbench-failure.png'),
            fullPage: true,
          })
        } catch (diagnosticFailure) {
          failures.push(diagnosticFailure)
        }
      }
      page.off('request', inspectRequest)
      page.off('pageerror', inspectError)
      page.off('response', inspectResponse)
      page.off('requestfailed', inspectFailed)
      for (const id of jobs) {
        try {
          await json(`/api/game-assets/jobs/${id}/stop`, 'POST', {}, 10000)
        } catch (failure) {
          failures.push(failure)
        }
      }
      for (const id of projects) {
        try {
          assert.equal((await json(`/api/game-assets/projects/${id}`, 'DELETE')).deleted, true)
        } catch (failure) {
          failures.push(failure)
        }
      }
      try {
        await page.setViewportSize({ width: 1440, height: 1000 })
      } catch (failure) {
        failures.push(failure)
      }
      if (failures.length) throw collectedFailure(failures)
      return evidence
    },
  )
}
