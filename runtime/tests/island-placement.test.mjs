import assert from 'node:assert/strict'
import test from 'node:test'
import {
  DEFAULT_FLOATING_PLACEMENT,
  defaultFloatingPlacement,
  FLOATING_PLACEMENT_LIMIT,
  floatingPlacementKey,
  floatingPositionPixels,
  moveFloatingPlacement,
  restoreFloatingPlacements,
  saveFloatingPlacement,
} from '../../src/features/custom-ui/floating-placement.ts'

const bounds = { width: 1000, height: 700 }
const widget = { width: 380, height: 88 }

test('default placement aligns the widget center with the page header and clamps near edges', () => {
  const smallWidget = { width: 380, height: 64 }
  const anchor = { x: 200, y: 24, width: 800, height: 80 }
  const position = defaultFloatingPlacement(bounds, smallWidget, anchor)
  assert.deepEqual(floatingPositionPixels(position, bounds, smallWidget), { x: 410, y: 32 })
  assert.deepEqual(defaultFloatingPlacement(bounds, smallWidget), DEFAULT_FLOATING_PLACEMENT)
  assert.deepEqual(
    defaultFloatingPlacement(bounds, smallWidget, { x: -500, y: -500, width: 1, height: 1 }),
    { x: 0, y: 0 },
  )
  assert.deepEqual(defaultFloatingPlacement({ width: 200, height: 40 }, smallWidget, anchor), {
    x: 0.5,
    y: 0,
  })
})

test('unpositioned widgets stack below the header and cascade inside a short viewport', () => {
  const anchor = { x: 0, y: 0, width: 1000, height: 52 }
  const card = { width: 380, height: 240 }
  const first = defaultFloatingPlacement(bounds, card, anchor, 1)
  const second = defaultFloatingPlacement(bounds, card, anchor, 2)
  const third = defaultFloatingPlacement(bounds, card, anchor, 3)
  assert.deepEqual(floatingPositionPixels(first, bounds, card), { x: 310, y: 64 })
  assert.deepEqual(floatingPositionPixels(second, bounds, card), { x: 310, y: 316 })
  assert.deepEqual(floatingPositionPixels(third, bounds, card), { x: 334, y: 460 })
  const short = { width: 400, height: 280 }
  const positions = [1, 2, 3].map((index) =>
    floatingPositionPixels(defaultFloatingPlacement(short, card, anchor, index), short, card),
  )
  assert.equal(new Set(positions.map((value) => `${value.x},${value.y}`)).size, 3)
  for (const position of positions) {
    assert.ok(position.x >= 0 && position.x <= 20)
    assert.ok(position.y >= 0 && position.y <= 40)
  }
})

test('floating widgets start at the top center and remain inside resized viewport bounds', () => {
  assert.deepEqual(floatingPositionPixels(DEFAULT_FLOATING_PLACEMENT, bounds, widget), {
    x: 310,
    y: 0,
  })
  assert.deepEqual(floatingPositionPixels({ x: 1, y: 1 }, bounds, widget), { x: 620, y: 612 })
  assert.deepEqual(floatingPositionPixels({ x: 1, y: 1 }, { width: 300, height: 80 }, widget), {
    x: 0,
    y: 0,
  })
  assert.deepEqual(
    floatingPositionPixels({ x: 0.5, y: 0.5 }, { width: 400, height: 300 }, widget),
    { x: 10, y: 106 },
  )
})

test('pointer and keyboard deltas use the same bounded normalized positioning', () => {
  const moved = moveFloatingPlacement(
    DEFAULT_FLOATING_PLACEMENT,
    { x: 124, y: 306 },
    bounds,
    widget,
  )
  assert.deepEqual(moved, { x: 0.7, y: 0.5 })
  assert.deepEqual(moveFloatingPlacement(moved, { x: 100000, y: -100000 }, bounds, widget), {
    x: 1,
    y: 0,
  })
  assert.deepEqual(moveFloatingPlacement(moved, { x: -100000, y: 100000 }, bounds, widget), {
    x: 0,
    y: 1,
  })
  const narrow = { width: 100, height: 50 }
  assert.deepEqual(moveFloatingPlacement(moved, { x: 12, y: 12 }, narrow, widget), moved)
  assert.deepEqual(floatingPositionPixels(moved, bounds, widget), { x: 434, y: 306 })
})

test('position storage isolates devices and instances, rejects malformed values and keeps recent entries', () => {
  const desktop = floatingPlacementKey('widget-a', false)
  const mobile = floatingPlacementKey('widget-a', true)
  assert.notEqual(desktop, mobile)
  let entries = saveFloatingPlacement([], desktop, { x: 0.25, y: 0.5 })
  entries = saveFloatingPlacement(entries, mobile, { x: 0.8, y: 0.1 })
  assert.equal(entries.length, 2)
  assert.deepEqual(restoreFloatingPlacements(JSON.parse(JSON.stringify(entries))), entries)
  const invalid = [
    null,
    {},
    { key: '__proto__', position: { x: 0, y: 0 } },
    { key: desktop, position: { x: NaN, y: 0 } },
    { key: desktop, position: { x: 0, y: Infinity } },
    { key: desktop, position: { x: '0', y: 0 } },
  ]
  assert.deepEqual(restoreFloatingPlacements(invalid), [])
  assert.deepEqual(restoreFloatingPlacements({ positions: entries }), [])
  assert.deepEqual(saveFloatingPlacement([], desktop, { x: -5, y: 7 }), [
    { key: desktop, position: { x: 0, y: 1 } },
  ])
  for (let index = 0; index < FLOATING_PLACEMENT_LIMIT + 10; index++)
    entries = saveFloatingPlacement(entries, floatingPlacementKey(`instance-${index}`, false), {
      x: 0.5,
      y: 0,
    })
  assert.equal(entries.length, FLOATING_PLACEMENT_LIMIT)
  assert.equal(entries[0].key, 'desktop:instance-10')
  entries = saveFloatingPlacement(entries, entries[0].key, { x: 1, y: 1 })
  assert.equal(entries.at(-1).key, 'desktop:instance-10')
  assert.equal(new Set(entries.map((entry) => entry.key)).size, FLOATING_PLACEMENT_LIMIT)
})
