import assert from 'node:assert/strict'
import { test } from 'node:test'
import { createRequire } from 'node:module'
import { mkdtemp, readFile, rm, watch, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createHash } from 'node:crypto'
import { PNG } from 'pngjs'
import jpeg from 'jpeg-js'
import { WorkflowImageProcessor } from '../services/workflow-image-processing.mjs'
import { SpriteEngineService } from '../services/sprite-engine-service.mjs'
import { normalizeWorkflowImageSettings } from '../../shared/workflow-image-nodes.mjs'
import {
  computeOpaqueBounds,
  detectOpaqueComponents,
} from '../../shared/vendor/framebaker/pixels.mjs'

const require = createRequire(import.meta.url)
const settings = (input = {}) => normalizeWorkflowImageSettings(input)
function fixture(width, height, pixel = () => [255, 0, 255, 255]) {
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
function processor(
  t,
  engines = {
    getExecutionDirectory: () => {
      throw new Error('Unexpected model access')
    },
  },
) {
  const value = new WorkflowImageProcessor({ engines })
  t.after(() => value.dispose())
  return value
}
const alpha = (frame, x, y) => PNG.sync.read(frame.buffer).data[(y * frame.width + x) * 4 + 3]

test('color node detects border color, preserves enclosed color, and supports whole-image removal', async (t) => {
  const value = processor(t)
  const source = fixture(5, 5, (x, y) =>
    x === 0 || y === 0 || x === 4 || y === 4 || (x === 2 && y === 2)
      ? [255, 0, 255, 255]
      : [20, 40, 80, 255],
  )
  const edge = await value.process({
    operation: 'background',
    frames: [source],
    settings: settings({ softness: 0 }),
  })
  assert.equal(alpha(edge.frames[0], 0, 0), 0)
  assert.equal(alpha(edge.frames[0], 2, 2), 255)
  const full = await value.process({
    operation: 'background',
    frames: [source],
    settings: settings({ edgeConnected: false }),
  })
  assert.equal(alpha(full.frames[0], 2, 2), 0)
  assert.equal(alpha(full.frames[0], 1, 1), 255)
  assert.equal(alpha(source, 0, 0), 255)
})

test('color node has adjustable soft edges without increasing original alpha', async (t) => {
  const value = processor(t)
  const result = await value.process({
    operation: 'background',
    frames: [fixture(2, 1, (x) => (x ? [225, 0, 255, 128] : [255, 0, 255, 255]))],
    settings: settings({ colors: ['#ff00ff'], tolerance: 20, softness: 20, edgeConnected: false }),
  })
  assert.equal(alpha(result.frames[0], 0, 0), 0)
  assert.equal(alpha(result.frames[0], 1, 0), 64)
})

test('generated atlas metadata takes precedence over node defaults and frame order is retained', async (t) => {
  const value = processor(t)
  const source = {
    ...fixture(4, 2, (x, y) => [x * 40, y * 90, 0, 255]),
    columns: 2,
    rows: 2,
    frameCount: 3,
  }
  const result = await value.process({
    operation: 'frames',
    frames: [source],
    settings: settings({ columns: 4, rows: 1, frameCount: 4 }),
  })
  assert.equal(result.frames.length, 3)
  assert.deepEqual(
    result.frames.map((frame) => [
      frame.width,
      frame.height,
      frame.columns,
      frame.rows,
      frame.frameCount,
    ]),
    [
      [2, 1, 1, 1, 1],
      [2, 1, 1, 1, 1],
      [2, 1, 1, 1, 1],
    ],
  )
  assert.deepEqual([...PNG.sync.read(result.frames[2].buffer).data.slice(0, 4)], [0, 90, 0, 255])
})

test('transform node reorders, disables, aligns, rotates and exports matching atlas metadata', async (t) => {
  const value = processor(t)
  const sources = [
    fixture(4, 4, (x, y) => (x === 1 && y > 0 ? [180, 10, 20, 255] : [0, 0, 0, 0])),
    fixture(4, 4, (x, y) => (x > 0 && y === 1 ? [20, 40, 200, 255] : [0, 0, 0, 0])),
    fixture(1, 1),
  ]
  const transformed = await value.process({
    operation: 'transform',
    frames: sources,
    settings: settings({
      padding: 0,
      frameOrder: [1, 0],
      transforms: [
        { index: 0, rotation: 90, opacity: 0.5, durationMs: 240 },
        { index: 2, enabled: false },
      ],
    }),
  })
  assert.equal(transformed.frames.length, 2)
  assert.deepEqual(
    transformed.frames.map((frame) => [frame.width, frame.height]),
    [
      [transformed.frames[0].width, transformed.frames[0].height],
      [transformed.frames[0].width, transformed.frames[0].height],
    ],
  )
  assert.equal(transformed.frames[1].durationMs, 240)
  const data = PNG.sync.read(transformed.frames[1].buffer).data
  assert.ok([...data].filter((_, index) => index % 4 === 3).includes(128))
  const output = await value.process({
    operation: 'export',
    frames: transformed.frames,
    settings: settings({ padding: 1 }),
  })
  assert.equal(output.atlas.frames.length, 2)
  assert.equal(output.atlas.frames[1].durationMs, 240)
  assert.equal(output.atlas.frames[0].action, 'walk')
  const atlas = PNG.sync.read(output.atlas.buffer)
  for (const [index, rect] of output.atlas.frames.entries()) {
    const pixels = PNG.sync.read(output.frames[index].buffer)
    for (let y = 0; y < rect.height; y++)
      assert.deepEqual(
        atlas.data.subarray(
          ((rect.y + y) * atlas.width + rect.x) * 4,
          ((rect.y + y) * atlas.width + rect.x + rect.width) * 4,
        ),
        pixels.data.subarray(y * rect.width * 4, (y + 1) * rect.width * 4),
      )
  }
})

test('background recommendation avoids the dominant character color', async (t) => {
  assert.equal(
    await processor(t).suggestBackground(fixture(4, 4, () => [255, 0, 255, 255])),
    '#00FF00',
  )
})

test('PNG, JPEG and WebP codecs run in the headless worker', async (t) => {
  const value = processor(t)
  const source = fixture(3, 2, () => [20, 80, 120, 255])
  const pixels = PNG.sync.read(source.buffer)
  const encodedJpeg = jpeg.encode(pixels, 95)
  const webp = await import('@jsquash/webp/encode.js')
  const { simd } = await import('wasm-feature-detect')
  const codec = (await simd()) ? 'webp_enc_simd.wasm' : 'webp_enc.wasm'
  await webp.init(
    await WebAssembly.compile(await readFile(require.resolve(`@jsquash/webp/codec/enc/${codec}`))),
  )
  const encodedWebp = await webp.default(pixels, { lossless: 1 })
  const result = await value.process({
    operation: 'export',
    frames: [
      source,
      { ...source, buffer: encodedJpeg.data, mimeType: 'image/jpeg' },
      { ...source, buffer: new Uint8Array(encodedWebp), mimeType: 'image/webp' },
    ],
    settings: settings({ padding: 0 }),
  })
  assert.equal(result.frames.length, 3)
  for (const frame of result.frames) {
    assert.equal(frame.mimeType, 'image/png')
    assert.deepEqual([frame.width, frame.height], [3, 2])
    assert.equal(alpha(frame, 0, 0), 255)
  }
})

test('processor rejects invalid dimensions, metadata, compressed corruption, and excessive output', async (t) => {
  const value = processor(t)
  const source = fixture(4, 4)
  await assert.rejects(
    value.process({ operation: 'export', frames: [{ ...source, width: 2 }], settings: settings() }),
    { code: 'workflow_image_invalid' },
  )
  await assert.rejects(
    value.process({ operation: 'frames', frames: [source], settings: settings({ columns: 16 }) }),
    { code: 'workflow_image_invalid_grid' },
  )
  const broken = Buffer.from(source.buffer)
  broken[broken.length - 5] ^= 1
  await assert.rejects(
    value.process({
      operation: 'export',
      frames: [{ ...source, buffer: broken }],
      settings: settings(),
    }),
    { code: 'workflow_image_processing_failed' },
  )
  await assert.rejects(
    value.process({
      operation: 'transform',
      frames: [source],
      settings: settings({
        transforms: [
          { index: 0, x: 4096 },
          { index: 1, x: -4096 },
        ],
      }),
    }),
    { code: 'workflow_image_invalid' },
  )
  await assert.rejects(
    value.process({
      operation: 'transform',
      frames: Array.from({ length: 20 }, () => fixture(256, 256)),
      settings: settings({
        maxFrameSize: 1024,
        transforms: Array.from({ length: 20 }, (_, index) => ({ index, scale: 8 })),
      }),
    }),
    { code: 'workflow_image_too_large' },
  )
})

test('shared frame downscaling preserves bottom alignment, offsets, and original pixel colors', async (t) => {
  const value = processor(t)
  const source = (top) =>
    fixture(128, 128, (x, y) => (x >= 32 && x < 96 && y >= top ? [40, 90, 180, 255] : [0, 0, 0, 0]))
  const result = await value.process({
    operation: 'transform',
    frames: [source(32), source(64)],
    settings: settings({ padding: 0, maxFrameSize: 32, transforms: [{ index: 1, x: 24 }] }),
  })
  assert.deepEqual(
    result.frames.map((frame) => [frame.width, frame.height]),
    [
      [30, 32],
      [30, 32],
    ],
  )
  const rects = result.frames.map((frame) => {
    const data = new Uint8ClampedArray(PNG.sync.read(frame.buffer).data)
    for (let offset = 0; offset < data.length; offset += 4)
      if (data[offset + 3])
        assert.deepEqual([...data.subarray(offset, offset + 4)], [40, 90, 180, 255])
    return computeOpaqueBounds(data, frame.width, frame.height)
  })
  assert.equal(rects[0].y + rects[0].h, rects[1].y + rects[1].h)
  assert.equal(rects[1].x - rects[0].x, 8)
  const small = await value.process({
    operation: 'transform',
    frames: [fixture(8, 6)],
    settings: settings({ padding: 0 }),
  })
  assert.deepEqual([small.frames[0].width, small.frames[0].height], [8, 6])
  const rotated = await value.process({
    operation: 'transform',
    frames: [fixture(64, 32, (x) => (x < 32 ? [200, 10, 20, 255] : [20, 10, 200, 255]))],
    settings: settings({
      padding: 0,
      maxFrameSize: 32,
      transforms: [{ index: 0, rotation: 90, scale: 2 }],
    }),
  })
  assert.deepEqual([rotated.frames[0].width, rotated.frames[0].height], [16, 32])
  const rotatedPixels = PNG.sync.read(rotated.frames[0].buffer).data
  assert.deepEqual([...rotatedPixels.subarray(0, 4)], [200, 10, 20, 255])
  assert.deepEqual([...rotatedPixels.subarray(rotatedPixels.length - 4)], [20, 10, 200, 255])
})

test(
  '1536x1024 generated direction sheets become 128 bounded frames and an exportable atlas',
  { timeout: 15000 },
  async (t) => {
    const value = processor(t)
    const sheet = {
      ...fixture(1536, 1024, (x, y) => [Math.floor(x / 384) * 50, y < 512 ? 80 : 160, 100, 255]),
      columns: 4,
      rows: 1,
      frameCount: 4,
    }
    const directions = ['S', 'SW', 'W', 'NW', 'N', 'NE', 'E', 'SE']
    const separated = await value.process({
      operation: 'frames',
      frames: directions.map((direction) => ({ ...sheet, direction })),
      settings: settings(),
    })
    assert.equal(separated.frames.length, 32)
    assert.ok(separated.frames.every((frame) => frame.width === 384 && frame.height === 1024))
    const normalized = await value.process({
      operation: 'transform',
      frames: separated.frames,
      settings: settings(),
    })
    const dimensions = new Set(normalized.frames.map((frame) => `${frame.width}x${frame.height}`))
    assert.equal(dimensions.size, 1)
    assert.ok(normalized.frames.every((frame) => frame.width <= 256 && frame.height <= 256))
    const allActions = ['idle', 'walk', 'run', 'attack'].flatMap((action) =>
      normalized.frames.map((frame) => ({ ...frame, action })),
    )
    assert.equal(allActions.length, 128)
    assert.ok(
      allActions.reduce((sum, frame) => sum + frame.width * frame.height, 0) <= 128 * 256 * 256,
    )
    const exported = await value.process({
      operation: 'export',
      frames: allActions,
      settings: settings(),
    })
    assert.equal(exported.atlas.frames.length, 128)
    assert.ok(exported.atlas.width <= 4096 && exported.atlas.height <= 4096)
    assert.ok(exported.atlas.width * exported.atlas.height <= 16_000_000)
    const decoded = PNG.sync.read(exported.atlas.buffer)
    assert.equal(decoded.width, exported.atlas.width)
    assert.equal(decoded.height, exported.atlas.height)
    for (const action of ['idle', 'walk', 'run', 'attack'])
      for (const direction of directions)
        assert.equal(
          exported.atlas.frames.filter(
            (frame) => frame.action === action && frame.direction === direction,
          ).length,
          4,
        )
  },
)

test('queued cancellation returns immediately and disposal prevents new processing', async (t) => {
  let unblock
  const blocked = new Promise((resolve) => {
    unblock = resolve
  })
  const value = processor(t, { getExecutionDirectory: () => blocked })
  const current = new AbortController()
  const first = value.process(
    { operation: 'background', frames: [fixture(2, 2)], settings: settings({ method: 'model' }) },
    { signal: current.signal },
  )
  const second = new AbortController()
  const queued = value.process(
    { operation: 'export', frames: [fixture(2, 2)], settings: settings() },
    { signal: second.signal },
  )
  second.abort()
  await assert.rejects(queued, { name: 'AbortError' })
  current.abort()
  await assert.rejects(first, { name: 'AbortError' })
  unblock('')
  await value.dispose()
  await assert.rejects(
    value.process({ operation: 'export', frames: [fixture(2, 2)], settings: settings() }),
    { code: 'workflow_image_closed' },
  )
})

test('execution directory revalidates cached files before exposing an engine to workers', async () => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-image-engine-'))
  const bytes = Buffer.from('fixed engine')
  const engine = new SpriteEngineService({
    dataDir,
    definitions: [
      {
        id: 'background',
        name: 'test',
        version: '1',
        licenses: [],
        files: [
          {
            name: 'engine.js',
            url: 'https://example.invalid/engine.js',
            bytes: bytes.length,
            sha256: createHash('sha256').update(bytes).digest('hex'),
            mimeType: 'text/javascript',
          },
        ],
      },
    ],
  })
  try {
    await engine.installBundleFiles({ 'engines/background/engine.js': bytes })
    const directory = await engine.getExecutionDirectory('background')
    await writeFile(join(directory, 'engine.js'), 'wrong engine')
    await assert.rejects(engine.getExecutionDirectory('background'), {
      code: 'sprite_engine_integrity',
    })
  } finally {
    await engine.dispose()
    await rm(dataDir, { recursive: true, force: true })
  }
})

test('inpaint modifies only the chosen region and retains the source alpha', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-inpaint-worker-'))
  const value = processor(t, { getExecutionDirectory: async () => directory })
  try {
    await writeFile(
      join(directory, 'opencv.js'),
      `class Mat {
      constructor(h=0,w=0,type=1){this.data=new Uint8Array(h*w*type)}
      static zeros(h,w,type){return new Mat(h,w,type)} delete(){}
    }
    module.exports={Mat,CV_8UC1:1,CV_8UC3:3,INPAINT_TELEA:1,inpaint(source,mask,result){result.data=new Uint8Array(source.data.length).fill(90)}}`,
    )
    const source = fixture(4, 4, () => [10, 20, 30, 128])
    const result = await value.process({
      operation: 'inpaint',
      frames: [source],
      settings: settings({ region: { x: 25, y: 25, width: 25, height: 25 } }),
    })
    const data = PNG.sync.read(result.frames[0].buffer).data
    assert.deepEqual([...data.subarray(0, 4)], [10, 20, 30, 128])
    assert.deepEqual([...data.subarray(20, 24)], [90, 90, 90, 128])
  } finally {
    await value.dispose()
    await rm(directory, { recursive: true, force: true })
  }
})

test(
  'cancelling an active blocked WASM worker terminates it and releases the next queued node',
  { timeout: 5000 },
  async () => {
    const directory = await mkdtemp(join(tmpdir(), 'pisper-cancel-worker-'))
    const value = new WorkflowImageProcessor({
      engines: { getExecutionDirectory: async () => directory },
    })
    const observer = new AbortController()
    const events = watch(directory, { signal: observer.signal })
    try {
      await writeFile(join(directory, 'u2netp.onnx'), 'fixture')
      await writeFile(
        join(directory, 'ort.wasm.min.js'),
        `const fs=require('node:fs'),path=require('node:path');
      module.exports={env:{wasm:{}},Tensor:class {},InferenceSession:{create:async()=>({inputNames:['in'],outputNames:['out'],release:async()=>{},run:async()=>{
        fs.writeFileSync(path.join(__dirname,'started'),'1');Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0);return {};
      }})}}`,
      )
      const controller = new AbortController()
      const first = value.process(
        {
          operation: 'background',
          frames: [fixture(2, 2)],
          settings: settings({ method: 'model' }),
        },
        { signal: controller.signal },
      )
      for await (const event of events) {
        if (event.filename === 'started') break
      }
      const next = value.process({
        operation: 'export',
        frames: [fixture(2, 2)],
        settings: settings({ padding: 0 }),
      })
      controller.abort()
      await assert.rejects(first, { name: 'AbortError' })
      assert.equal((await next).frames.length, 1)
    } finally {
      observer.abort()
      await value.dispose()
      await rm(directory, { recursive: true, force: true })
    }
  },
)

test('vendored opaque analysis excludes isolated noise and returns foreground bounds', () => {
  const frame = fixture(8, 8, (x, y) =>
    (x < 3 && y < 3) || (x === 7 && y === 7) ? [20, 40, 60, 255] : [0, 0, 0, 0],
  )
  const data = new Uint8ClampedArray(PNG.sync.read(frame.buffer).data)
  assert.deepEqual(computeOpaqueBounds(data, 8, 8), { x: 0, y: 0, w: 8, h: 8 })
  assert.deepEqual(detectOpaqueComponents(data, 8, 8, { minAreaPixels: 2 }), [
    { x: 0, y: 0, w: 3, h: 3 },
  ])
})
