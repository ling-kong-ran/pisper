import assert from 'node:assert/strict'
import { test } from 'node:test'
import { PNG } from 'pngjs'
import { applyImageAlphaStrokes } from '../../shared/image-alpha-strokes.mjs'
import { normalizeImageFrameEdits } from '../../shared/image-frame-edits.mjs'
import { normalizeWorkflowImageSettings } from '../../shared/workflow-image-nodes.mjs'
import { WorkflowImageProcessor } from '../services/workflow-image-processing.mjs'

const settings = normalizeWorkflowImageSettings({ padding: 0, trim: false, align: 'none' })
const edits = (frames) => normalizeImageFrameEdits({ frames })
function fixture(width = 16, height = 16, pixel = () => [30, 70, 100, 200]) {
  const data = Buffer.alloc(width * height * 4)
  for (let y = 0; y < height; y++)
    for (let x = 0; x < width; x++) data.set(pixel(x, y), (y * width + x) * 4)
  return {
    buffer: PNG.sync.write({ width, height, data }),
    mimeType: 'image/png',
    width,
    height,
    durationMs: 125,
    action: 'walk',
    direction: 'S',
    columns: 1,
    rows: 1,
    frameCount: 1,
  }
}
function processor(t) {
  const instance = new WorkflowImageProcessor({
    engines: {
      getExecutionDirectory() {
        throw new Error('Manual edits must not load AI engines')
      },
    },
  })
  t.after(() => instance.dispose())
  return instance
}
const pixels = (frame) => PNG.sync.read(frame.buffer).data
const pixel = (frame, x, y) => [
  ...pixels(frame).subarray((y * frame.width + x) * 4, (y * frame.width + x) * 4 + 4),
]

test('manual edit normalization rejects non-finite transforms, invalid indices and brush budgets', () => {
  const source = {
    frames: [{ sourceIndex: 0, eraseStrokes: [{ radius: 0.1, points: [{ x: 0.5, y: 0.5 }] }] }],
  }
  const result = normalizeImageFrameEdits(source)
  assert.equal(result.frames[0].scale, 1)
  assert.equal(result.frames[0].durationMs, 125)
  result.frames[0].eraseStrokes[0].points[0].x = 0
  assert.equal(source.frames[0].eraseStrokes[0].points[0].x, 0.5)
  const invalid = [null, {}, { frames: [] }, { frames: Array(513).fill({ sourceIndex: 0 }) }]
  for (const value of invalid)
    assert.throws(() => normalizeImageFrameEdits(value), { code: 'workflow_image_invalid_edits' })
  for (const [key, values] of Object.entries({
    sourceIndex: [undefined, -1, 512, 0.5, '0'],
    x: [Infinity, NaN, -4097],
    y: [-Infinity, 4097],
    scale: [0, -1, 9],
    rotation: [361, NaN],
    opacity: [-0.1, 1.1],
    durationMs: [15, 10001, 120.5],
  }))
    for (const value of values)
      assert.throws(() => edits([{ sourceIndex: 0, [key]: value }]), {
        code: 'workflow_image_invalid_edits',
      })
  const point = { x: 0.5, y: 0.5 }
  for (const stroke of [
    { points: [point] },
    { radius: 0.3, points: [point] },
    { radius: 0.1, points: [] },
    { radius: 0.1, points: [{ x: 1.1, y: 0 }] },
    { radius: 0.1, points: [point], restore: 'true' },
    { radius: 0.1, points: Array(513).fill(point) },
  ])
    assert.throws(() => edits([{ sourceIndex: 0, eraseStrokes: [stroke] }]), {
      code: 'workflow_image_invalid_edits',
    })
  assert.throws(
    () =>
      edits([{ sourceIndex: 0, eraseStrokes: Array(65).fill({ radius: 0.1, points: [point] }) }]),
    { code: 'workflow_image_invalid_edits' },
  )
  assert.throws(
    () =>
      edits([
        {
          sourceIndex: 0,
          eraseStrokes: Array(40).fill({ radius: 0.1, points: Array(512).fill(point) }),
        },
      ]),
    { code: 'workflow_image_too_large' },
  )
})

test('manual brush connects sparse points, preserves RGB, and restores only original alpha', async (t) => {
  const value = processor(t)
  const source = fixture(16, 16, (x) => [30, 70, 100, x === 8 ? 0 : 200])
  const original = Buffer.from(source.buffer)
  const stroke = {
    radius: 0.04,
    points: [
      { x: 1.5 / 16, y: 8.5 / 16 },
      { x: 14.5 / 16, y: 8.5 / 16 },
    ],
  }
  const erased = await value.process({
    operation: 'edit',
    frames: [source],
    settings,
    edits: edits([{ sourceIndex: 0, eraseStrokes: [stroke] }]),
  })
  for (let x = 1; x < 15; x++) assert.deepEqual(pixel(erased.frames[0], x, 8), [30, 70, 100, 0])
  assert.deepEqual(pixel(erased.frames[0], 2, 7), [30, 70, 100, 200])
  const restored = await value.process({
    operation: 'edit',
    frames: [source],
    settings,
    edits: edits([{ sourceIndex: 0, eraseStrokes: [stroke, { ...stroke, restore: true }] }]),
  })
  assert.deepEqual(pixels(restored.frames[0]), pixels(source))
  assert.deepEqual(pixel(restored.frames[0], 8, 8), [30, 70, 100, 0])
  assert.deepEqual(source.buffer, original)
})

test('manual edits independently duplicate, drop and reorder frames with per-frame durations and transforms', async (t) => {
  const value = processor(t)
  const first = fixture(4, 2, () => [200, 30, 40, 255])
  const removed = fixture(4, 2, () => [0, 255, 0, 255])
  const last = fixture(4, 2, () => [20, 70, 220, 255])
  const result = await value.process({
    operation: 'edit',
    frames: [first, removed, last],
    settings,
    edits: edits([
      { sourceIndex: 2, durationMs: 240 },
      { sourceIndex: 0, rotation: 90, scale: 2, opacity: 0.5, x: 1, y: 0, durationMs: 500 },
      {
        sourceIndex: 2,
        durationMs: 60,
        eraseStrokes: [{ radius: 0.25, points: [{ x: 0.125, y: 0.25 }] }],
      },
    ]),
  })
  assert.deepEqual(
    result.frames.map((frame) => frame.durationMs),
    [240, 500, 60],
  )
  assert.equal(new Set(result.frames.map((frame) => `${frame.width}:${frame.height}`)).size, 1)
  assert.equal(result.frames[0].height, 2)
  assert.ok(pixels(result.frames[1]).some((channel, index) => index % 4 === 3 && channel === 128))
  assert.notDeepEqual(pixels(result.frames[0]), pixels(result.frames[2]))
  assert.ok(
    result.frames.every(
      (frame) =>
        !pixels(frame).some(
          (channel, index, data) => index % 4 === 0 && channel === 0 && data[index + 1] === 255,
        ),
    ),
  )
  assert.deepEqual(pixel(first, 0, 0), [200, 30, 40, 255])
})

test('manual movement uses a fixed original canvas and clips overflow instead of recentering', async (t) => {
  const value = processor(t)
  const source = fixture(6, 6, (x, y) => (x === 2 && y === 2 ? [200, 20, 50, 255] : [0, 0, 0, 0]))
  const result = await value.process({
    operation: 'edit',
    frames: [source],
    settings,
    edits: edits([{ sourceIndex: 0, x: 1, y: 2 }]),
  })
  assert.deepEqual([result.frames[0].width, result.frames[0].height], [6, 6])
  assert.deepEqual(pixel(result.frames[0], 3, 4), [200, 20, 50, 255])
  assert.equal(pixel(result.frames[0], 2, 2)[3], 0)
  const clipped = await value.process({
    operation: 'edit',
    frames: [source],
    settings,
    edits: edits([{ sourceIndex: 0, x: 8 }]),
  })
  assert.ok(pixels(clipped.frames[0]).every((channel, index) => index % 4 !== 3 || channel === 0))
  assert.deepEqual(pixel(source, 2, 2), [200, 20, 50, 255])
})

test('manual rotation and scaling preserve pixel positions and dimensions from the preview canvas', async (t) => {
  const value = processor(t)
  const source = fixture(6, 6, (x, y) =>
    y === 2 && (x === 2 || x === 3)
      ? x === 2
        ? [200, 20, 50, 255]
        : [20, 70, 200, 255]
      : [0, 0, 0, 0],
  )
  const result = await value.process({
    operation: 'edit',
    frames: [source],
    settings,
    edits: edits([{ sourceIndex: 0, rotation: 90, scale: 2, opacity: 0.5 }]),
  })
  assert.deepEqual([result.frames[0].width, result.frames[0].height], [6, 6])
  for (const x of [3, 4])
    for (const y of [1, 2]) assert.deepEqual(pixel(result.frames[0], x, y), [200, 20, 50, 128])
  for (const x of [3, 4])
    for (const y of [3, 4]) assert.deepEqual(pixel(result.frames[0], x, y), [20, 70, 200, 128])
  assert.equal(pixel(result.frames[0], 2, 2)[3], 0)
  const large = fixture(32, 20)
  const varied = await value.process({
    operation: 'edit',
    frames: [source, large],
    settings: normalizeWorkflowImageSettings({ maxFrameSize: 16, padding: 64 }),
    edits: edits([{ sourceIndex: 1 }, { sourceIndex: 0 }]),
  })
  assert.deepEqual(
    varied.frames.map((frame) => [frame.width, frame.height]),
    [
      [32, 20],
      [6, 6],
    ],
  )
})

test('invalid edits do not poison subsequent processing and aborted requests leave source frames intact', async (t) => {
  const value = processor(t)
  const source = fixture()
  await assert.rejects(
    value.process({
      operation: 'edit',
      frames: [source],
      settings,
      edits: edits([{ sourceIndex: 1 }]),
    }),
    { code: 'workflow_image_invalid_edits' },
  )
  const controller = new AbortController()
  controller.abort()
  await assert.rejects(
    value.process(
      { operation: 'edit', frames: [source], settings, edits: edits([{ sourceIndex: 0 }]) },
      { signal: controller.signal },
    ),
    { name: 'AbortError' },
  )
  const success = await value.process({
    operation: 'edit',
    frames: [source],
    settings,
    edits: edits([{ sourceIndex: 0 }]),
  })
  assert.deepEqual(pixels(success.frames[0]), pixels(source))
})

test('copies are limited before allocating full decoded frames', async (t) => {
  const source = fixture(256, 256)
  await assert.rejects(
    processor(t).process({
      operation: 'edit',
      frames: [source],
      settings,
      edits: edits(Array(256).fill({ sourceIndex: 0 })),
    }),
    { code: 'workflow_image_too_large' },
  )
})

test('saved brush alpha matches the shared preview algorithm for fine diagonal, edge, and restore strokes', async (t) => {
  const source = fixture(17, 11, (x, y) => [30, 70, 100, (x * 31 + y * 17) % 256])
  const strokes = [
    {
      radius: 0.001,
      points: [
        { x: 0.5 / 17, y: 0.5 / 11 },
        { x: 16.5 / 17, y: 10.5 / 11 },
      ],
    },
    {
      radius: 0.04,
      points: [
        { x: 0, y: 0.3 },
        { x: 0.43, y: 0.97 },
        { x: 1, y: 0 },
      ],
    },
    {
      radius: 0.1,
      restore: true,
      points: [
        { x: 0.4, y: 0.3 },
        { x: 0.6, y: 0.7 },
      ],
    },
    { radius: 0.25, points: [{ x: 0, y: 1 }] },
  ]
  const input = edits([{ sourceIndex: 0, eraseStrokes: strokes }])
  const decoded = { width: 17, height: 11, data: new Uint8ClampedArray(pixels(source)) }
  const expected = applyImageAlphaStrokes(decoded, input.frames[0].eraseStrokes)
  const result = await processor(t).process({
    operation: 'edit',
    frames: [source],
    settings,
    edits: input,
  })
  assert.deepEqual(Array.from(pixels(result.frames[0])), Array.from(expected))
})
