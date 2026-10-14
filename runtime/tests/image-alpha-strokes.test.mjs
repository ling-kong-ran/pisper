import assert from 'node:assert/strict'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { applyImageAlphaStrokes } from '../../shared/image-alpha-strokes.mjs'

function frame(width = 17, height = 11) {
  const data = new Uint8ClampedArray(width * height * 4)
  for (let y = 0; y < height; y++)
    for (let x = 0; x < width; x++)
      data.set([15, 70, 220, (x * 31 + y * 17) % 256], (y * width + x) * 4)
  return { width, height, data }
}
const strokes = [
  {
    radius: 0.001,
    restore: false,
    points: [
      { x: 0.5 / 17, y: 0.5 / 11 },
      { x: 16.5 / 17, y: 10.5 / 11 },
    ],
  },
  {
    radius: 0.037,
    restore: false,
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
  { radius: 0.25, restore: false, points: [{ x: 0, y: 1 }] },
  { radius: 0.02, restore: true, points: [{ x: 0.5, y: 0.5 }] },
]

test('subpixel brush hits pixel centers without introducing anti-aliased partial alpha', () => {
  const source = { width: 3, height: 3, data: new Uint8ClampedArray(3 * 3 * 4).fill(255) }
  const edits = [
    {
      radius: 0.001,
      restore: false,
      points: [
        { x: 1 / 6, y: 1 / 6 },
        { x: 5 / 6, y: 5 / 6 },
      ],
    },
  ]
  const result = applyImageAlphaStrokes(source, edits)
  for (let y = 0; y < 3; y++)
    for (let x = 0; x < 3; x++) assert.equal(result[(y * 3 + x) * 4 + 3], x === y ? 0 : 255)
  assert.deepEqual(
    applyImageAlphaStrokes(source, [...edits, { ...edits[0], restore: true }]),
    source.data,
  )
})

test('preview and worker share a cumulative brush work budget without modifying source pixels', () => {
  const source = frame()
  const original = new Uint8ClampedArray(source.data)
  const budget = { remaining: 0 }
  assert.throws(() => applyImageAlphaStrokes(source, strokes, budget), {
    code: 'workflow_image_too_large',
  })
  assert.deepEqual(source.data, original)
  const browserFunction = runInNewContext(`(${applyImageAlphaStrokes.toString()})`)
  assert.throws(() => browserFunction(source, strokes, { remaining: 0 }), {
    code: 'workflow_image_too_large',
  })
})
