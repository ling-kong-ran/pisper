import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { PNG } from 'pngjs'
import {
  buildActionSheetPrompt,
  buildActionVideoPrompt,
  buildCharacterDirectionSheetPrompt,
  suggestActionSheetGrid,
} from '../../shared/vendor/framebaker/action-prompts.mjs'
import { WorkflowMediaService } from '../services/workflow-media-service.mjs'
import { ImageOperationService } from '../services/image-operation-service.mjs'
import { WorkflowImageNodeService } from '../services/workflow-image-node-service.mjs'
import { WorkflowImageProcessor } from '../services/workflow-image-processing.mjs'

function png(width = 64, height = 16) {
  const image = new PNG({ width, height })
  image.data.fill(255)
  return PNG.sync.write(image)
}

async function fixture(t, overrides = {}) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-image-nodes-'))
  const media = new WorkflowMediaService({ dataDir: root })
  const reference = await media.upload({
    name: 'character.png',
    mimeType: 'image/png',
    buffer: png(16, 16),
  })
  const calls = []
  const processing = []
  const processor = {
    async suggestBackground() {
      return '#00FF00'
    },
    async process(request) {
      processing.push(request)
      if (request.operation === 'export')
        return {
          frames: request.frames,
          atlas: {
            buffer: png(32, 16),
            width: 32,
            height: 16,
            frames: request.frames.map((frame, index) => ({
              x: index * 16,
              y: 0,
              width: 16,
              height: 16,
              durationMs: frame.durationMs,
              action: frame.action,
              direction: frame.direction,
            })),
          },
        }
      return {
        frames: request.frames.map((frame) => ({
          ...frame,
          buffer: png(16, 16),
          width: 16,
          height: 16,
          columns: 1,
          rows: 1,
          frameCount: 1,
        })),
      }
    },
  }
  const generateImage = async (request, options) => {
    calls.push({ request, options })
    await mkdir(join(request.cwd, 'generated', 'visuals'), { recursive: true })
    const path = join(request.cwd, 'generated', 'visuals', `${request.outputName}.png`)
    await writeFile(path, png())
    return { path, mimeType: 'image/png' }
  }
  const operations = new ImageOperationService({
    dataDir: root,
    media,
    processor,
    generateImage,
    ...overrides,
  })
  const service = new WorkflowImageNodeService({ operations })
  t.after(async () => {
    await operations.dispose()
    await media.dispose()
    await rm(root, { recursive: true, force: true })
  })
  const execute = (kind, previous = [], image = {}, extra = {}) =>
    service.execute({
      node: { id: kind, kind, label: kind, prompt: '', image, ...extra.node },
      inputs: { reference },
      predecessors: previous.map((output) => ({ output })),
      workflowId: 'workflow',
      runId: 'run',
      cwd: '/must-not-write-to-user-workspace',
      ...extra,
    })
  return { root, media, reference, service, operations, calls, processing, processor, execute }
}

test('FrameBaker prompt helpers retain reference style and ordered short animation sequences', () => {
  assert.deepEqual(suggestActionSheetGrid(4), { cols: 4, rows: 1 })
  assert.deepEqual(suggestActionSheetGrid(8), { cols: 4, rows: 2 })
  assert.deepEqual(suggestActionSheetGrid(100), { cols: 4, rows: 4 })
  const frames = Array.from({ length: 8 }, () => ({ id: 'walk', label: 'walk', prompt: 'walk' }))
  const prompt = buildActionSheetPrompt({ frames, cols: 4, rows: 2, extra: 'detail '.repeat(500) })
  assert.match(prompt, /8-frame continuous walk cycle/)
  assert.match(prompt, /last loops to first/)
  assert.match(prompt, /art style as reference/)
  assert.ok(prompt.length <= 1400)
  assert.match(buildCharacterDirectionSheetPrompt({}), /center EMPTY/)
  assert.match(buildActionVideoPrompt({ actions: frames }), /15% empty safe margin/)
  assert.doesNotMatch(buildActionVideoPrompt({ actions: [] }), /pixel art/i)
})

test('image DAG passes real reference bytes, generates per direction and preserves typed results through preview/export', async (t) => {
  const value = await fixture(t)
  const input = await value.execute('media-input')
  const generated = await value.execute('media-generate', [input.output], {
    action: 'walk',
    directions: ['S', 'W'],
    frameCount: 4,
  })
  assert.equal(value.calls.length, 2)
  for (const [index, { request, options }] of value.calls.entries()) {
    assert.equal(request.model, '')
    assert.equal(options.allowFallback, false)
    assert.equal(request.aspectRatio, '16:9')
    assert.notEqual(request.cwd, '/must-not-write-to-user-workspace')
    assert.equal(request.sourceImages.length, 1)
    assert.match(request.prompt, /4-frame continuous walk cycle/)
    assert.match(request.prompt, /left heel contact/)
    assert.match(request.prompt, /15% safe margin/)
    assert.match(request.prompt, /Solid #00FF00/)
    assert.match(request.prompt, index === 0 ? /faces FRONT / : /faces LEFT profile/)
  }
  assert.deepEqual(
    generated.output.frames.map((frame) => frame.direction),
    ['S', 'W'],
  )
  assert.deepEqual(
    generated.output.frames.map((frame) => [frame.columns, frame.rows, frame.frameCount]),
    [
      [4, 1, 4],
      [4, 1, 4],
    ],
  )
  assert.deepEqual(await readdir(join(value.root, 'image-operation-jobs')), [])
  const background = await value.execute('media-background', [generated.output])
  const split = await value.execute('media-frames', [background.output])
  const transformed = await value.execute('media-transform', [split.output], { frameOrder: [1, 0] })
  const preview = await value.execute('media-preview', [transformed.output])
  assert.deepEqual(preview.output, transformed.output)
  const exported = await value.execute('media-export', [preview.output], { filename: 'walk' })
  assert.deepEqual(exported.output.frames, preview.output.frames)
  assert.equal(exported.output.atlas.media.name, 'walk.png')
  assert.equal(exported.output.atlas.frames.length, 2)
  assert.deepEqual(
    value.processing.map((entry) => entry.operation),
    ['background', 'frames', 'transform', 'export'],
  )
  assert.doesNotMatch(JSON.stringify(exported), /must-not-write|pisper-image-nodes/)
})

test('generation stages the authenticated original image with a usable extension and honors explicit model selection', async (t) => {
  let sourceBytes
  let seenModel
  const value = await fixture(t, {
    generateImage: async (request) => {
      sourceBytes = await readFile(request.sourceImages[0])
      seenModel = request.model
      assert.match(request.sourceImages[0], /reference-0\.png$/)
      assert.match(request.prompt, /custom prompt/)
      await mkdir(join(request.cwd, 'generated', 'visuals'), { recursive: true })
      const path = join(request.cwd, 'generated', 'visuals', 'result.png')
      await writeFile(path, png(64, 32))
      return { path, mimeType: 'image/png' }
    },
  })
  const input = await value.execute('media-input')
  const result = await value.service.execute({
    node: {
      id: 'generate',
      kind: 'media-generate',
      prompt: 'custom prompt',
      model: { provider: 'provider', model: 'image-model' },
      image: { frameCount: 8 },
    },
    inputs: {},
    predecessors: [{ output: input.output }],
    workflowId: 'w',
    runId: 'r',
  })
  assert.deepEqual(sourceBytes, png(16, 16))
  assert.equal(seenModel, 'provider/image-model')
  assert.equal(result.output.frames[0].rows, 2)
})

test('later direction failures preserve completed output without leaking upstream secrets or retrying', async (t) => {
  let attempts = 0
  const value = await fixture(t, {
    generateImage: async (request) => {
      attempts++
      if (attempts === 2) throw new Error('secret token and personal filesystem path')
      await mkdir(join(request.cwd, 'generated', 'visuals'), { recursive: true })
      const path = join(request.cwd, 'generated', 'visuals', 'result.png')
      await writeFile(path, png())
      return { path, mimeType: 'image/png' }
    },
  })
  const input = await value.execute('media-input')
  await assert.rejects(
    value.execute('media-generate', [input.output], { directions: ['S', 'W', 'N'] }),
    (error) => {
      assert.equal(error.code, 'workflow_image_generation_failed')
      assert.equal(error.message, error.code)
      assert.equal(error.partialOutput.frames.length, 1)
      assert.equal(error.partialOutput.frames[0].direction, 'S')
      assert.doesNotMatch(JSON.stringify(error), /secret|personal/)
      return true
    },
  )
  assert.equal(attempts, 2)
  assert.deepEqual(await readdir(join(value.root, 'image-operation-jobs')), [])
})

test('manually resuming a failed direction batch reuses completed media and requests only missing directions', async (t) => {
  const attempted = []
  const value = await fixture(t, {
    generateImage: async (request) => {
      attempted.push(request.outputName)
      if (attempted.length === 2) throw new Error('temporary model failure')
      await mkdir(join(request.cwd, 'generated', 'visuals'), { recursive: true })
      const path = join(request.cwd, 'generated', 'visuals', 'result.png')
      await writeFile(path, png())
      return { path, mimeType: 'image/png' }
    },
  })
  const input = await value.execute('media-input')
  const settings = { directions: ['S', 'W', 'N'], action: 'walk' }
  let resumeOutput
  await assert.rejects(value.execute('media-generate', [input.output], settings), (error) => {
    resumeOutput = error.partialOutput
    assert.equal(resumeOutput.frames.length, 1)
    return error.code === 'workflow_image_generation_failed'
  })
  const completed = await value.execute('media-generate', [input.output], settings, {
    resumeOutput,
  })
  assert.deepEqual(attempted, ['action-S', 'action-W', 'action-W', 'action-N'])
  assert.deepEqual(
    completed.output.frames.map((frame) => frame.direction),
    ['S', 'W', 'N'],
  )
  assert.deepEqual(completed.output.frames[0], resumeOutput.frames[0])
  const reused = await value.execute('media-generate', [input.output], settings, {
    resumeOutput: completed.output,
  })
  assert.deepEqual(reused.output, completed.output)
  assert.equal(attempted.length, 4)
  await value.execute('media-generate', [input.output], settings)
  assert.deepEqual(attempted.slice(4), ['action-S', 'action-W', 'action-N'])
})

test('resume rejects forged media and incompatible direction metadata before invoking a model', async (t) => {
  const value = await fixture(t)
  const input = await value.execute('media-input')
  const settings = { action: 'walk', directions: ['S', 'W'] }
  const first = await value.execute('media-generate', [input.output], settings)
  const partial = { ...first.output, frames: first.output.frames.slice(0, 1) }
  const cases = [
    {
      ...partial,
      frames: [{ ...partial.frames[0], media: { ...partial.frames[0].media, size: 1 } }],
    },
    ...[
      { direction: 'N' },
      { action: 'run' },
      { columns: 2 },
      { rows: 2 },
      { frameCount: 8 },
      { width: 1 },
      { durationMs: 500 },
    ].map((patch) => ({ ...partial, frames: [{ ...partial.frames[0], ...patch }] })),
    { ...partial, frames: [partial.frames[0], partial.frames[0]] },
  ]
  const callsBefore = value.calls.length
  for (const resumeOutput of cases) {
    await assert.rejects(
      value.execute('media-generate', [input.output], settings, { resumeOutput }),
      (error) => {
        assert.ok(['workflow_image_invalid', 'workflow_media_invalid'].includes(error.code))
        assert.equal(error.partialOutput, undefined)
        return true
      },
    )
  }
  assert.equal(value.calls.length, callsBefore)
})

test('media references cannot forge MIME, size, name or decoded dimensions', async (t) => {
  const value = await fixture(t)
  for (const patch of [{ name: 'forged.png' }, { size: 1 }, { mimeType: 'image/jpeg' }]) {
    await assert.rejects(
      value.execute(
        'media-input',
        [],
        {},
        { inputs: { reference: { ...value.reference, ...patch } } },
      ),
      { code: 'workflow_media_invalid' },
    )
  }
  const input = await value.execute('media-input')
  input.output.frames[0].width = 42
  await assert.rejects(value.execute('media-preview', [input.output]), {
    code: 'workflow_image_invalid',
  })
  await assert.rejects(value.execute('media-preview'), { code: 'workflow_image_source_required' })
})

test('generation rejects escaped output paths and symlinks and always removes staging directories', async (t) => {
  let outside
  const value = await fixture(t, {
    generateImage: async (request) => {
      const directory = join(request.cwd, 'generated', 'visuals')
      await mkdir(directory, { recursive: true })
      const path = join(directory, 'escaped.png')
      await symlink(outside, path)
      return { path, mimeType: 'image/png' }
    },
  })
  outside = join(value.root, 'outside.png')
  await writeFile(outside, png())
  const input = await value.execute('media-input')
  await assert.rejects(value.execute('media-generate', [input.output]), {
    code: 'workflow_image_invalid',
  })
  value.operations.generateImage = async () => ({ path: outside, mimeType: 'image/png' })
  await assert.rejects(value.execute('media-generate', [input.output]), {
    code: 'workflow_image_invalid',
  })
  assert.deepEqual(await readdir(join(value.root, 'image-operation-jobs')), [])
  assert.deepEqual(await readFile(outside), png())
})

test('cancellation and disposal abort pending generation and prevent subsequent calls', async (t) => {
  let entered
  const pending = new Promise((resolve) => {
    entered = resolve
  })
  const value = await fixture(t, {
    generateImage: (_request, { signal }) =>
      new Promise((_resolve, reject) => {
        entered()
        signal.addEventListener('abort', () => reject(new Error('cancelled upstream')), {
          once: true,
        })
      }),
  })
  const input = await value.execute('media-input')
  const generating = value.execute('media-generate', [input.output])
  const rejected = assert.rejects(generating, { code: 'workflow_image_cancelled' })
  await pending
  await value.operations.dispose()
  await rejected
  await assert.rejects(value.execute('media-input'), { code: 'workflow_image_closed' })
  assert.deepEqual(await readdir(join(value.root, 'image-operation-jobs')), [])
})

test('real FrameBaker worker composes generation, local color removal, slicing, frame edits and atlas export without an Agent or downloads', async (t) => {
  const processor = new WorkflowImageProcessor({
    engines: { getExecutionDirectory: () => Promise.reject(new Error('unexpected download')) },
  })
  t.after(() => processor.dispose())
  const value = await fixture(t, {
    processor,
    generateImage: async (request) => {
      const sheet = new PNG({ width: 64, height: 16 })
      for (let y = 0; y < 16; y++)
        for (let x = 0; x < 64; x++) {
          const offset = (y * 64 + x) * 4
          const actor = x % 16 >= 4 && x % 16 < 12 && y >= 3 && y < 14
          sheet.data[offset] = actor ? 200 : 0
          sheet.data[offset + 1] = actor ? 0 : 255
          sheet.data[offset + 2] = actor ? 40 + Math.floor(x / 16) * 20 : 0
          sheet.data[offset + 3] = 255
        }
      await mkdir(join(request.cwd, 'generated', 'visuals'), { recursive: true })
      const path = join(request.cwd, 'generated', 'visuals', 'sheet.png')
      await writeFile(path, PNG.sync.write(sheet))
      return { path, mimeType: 'image/png' }
    },
  })
  const source = await value.execute('media-input')
  const generated = await value.execute('media-generate', [source.output], {
    directions: ['S'],
    frameCount: 4,
    colors: ['#00FF00'],
  })
  const removed = await value.execute('media-background', [generated.output], {
    method: 'color',
    colors: ['#00FF00'],
    tolerance: 1,
    softness: 0,
  })
  const split = await value.execute('media-frames', [removed.output], { trim: true, padding: 0 })
  assert.equal(split.output.frames.length, 4)
  const edited = await value.execute('media-transform', [split.output], {
    frameOrder: [3, 1],
    transforms: [
      { index: 0, enabled: false },
      { index: 3, durationMs: 200, x: 1 },
    ],
  })
  assert.equal(edited.output.frames.length, 3)
  assert.equal(edited.output.frames[0].durationMs, 200)
  const exported = await value.execute('media-export', [edited.output])
  assert.equal(exported.output.atlas.frames.length, 3)
  const result = await value.media.load(exported.output.atlas.media.id)
  const decoded = PNG.sync.read(result.buffer)
  assert.ok(decoded.data.some((component, index) => index % 4 === 3 && component === 0))
  assert.ok(decoded.data.some((component, index) => index % 4 === 3 && component === 255))
})
