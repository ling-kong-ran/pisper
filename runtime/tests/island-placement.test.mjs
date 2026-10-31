import assert from 'node:assert/strict'
import test from 'node:test'
import {
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

// 评审返修（A7）：未定位的浮窗停在右下角、向上叠，一列放不下向左错开，不再压在页头中线下方的第一屏内容上；
// 底部让出 160px 输入框区，不压住发送键。
test('unpositioned floating widgets start bottom-right above the composer and stack upward', () => {
  const card = { width: 260, height: 96 }
  const pixels = (index, area = bounds) =>
    floatingPositionPixels(defaultFloatingPlacement(area, card, index), area, card)
  assert.deepEqual(pixels(0), { x: 732, y: 444 })
  assert.deepEqual(pixels(1), { x: 732, y: 336 })
  assert.deepEqual(pixels(2), { x: 732, y: 228 })
  // 一列放 5 个（(700 - 96 - 160 - 8) / 108 + 1）；第 6 个起向左错开一列。
  assert.deepEqual(pixels(5), { x: 460, y: 444 })
  // 底边都在输入框区之上。
  for (const index of [0, 1, 2, 3, 4]) assert.ok(pixels(index).y + card.height <= 700 - 160)
  // 可用高度不足输入框区：退到顶部。
  assert.equal(pixels(0, { width: 1000, height: 200 }).y, 0)
  const positions = [0, 1, 2, 3, 4, 5, 6, 7].map((index) => pixels(index))
  assert.equal(new Set(positions.map((value) => `${value.x},${value.y}`)).size, positions.length)
  for (const position of positions) {
    assert.ok(position.x >= 0 && position.x <= bounds.width - card.width)
    assert.ok(position.y >= 0 && position.y <= bounds.height - card.height)
  }
  // 区域比浮窗还小或还没量到尺寸：贴右下（归一化 1, 1），不出现 NaN。
  assert.deepEqual(defaultFloatingPlacement({ width: 200, height: 40 }, card), { x: 1, y: 1 })
  assert.deepEqual(defaultFloatingPlacement({ width: 0, height: 0 }, { width: 0, height: 0 }), {
    x: 1,
    y: 1,
  })
})

test('floating positions remain inside resized viewport bounds', () => {
  assert.deepEqual(floatingPositionPixels({ x: 0.5, y: 0 }, bounds, widget), {
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
  const moved = moveFloatingPlacement({ x: 0.5, y: 0 }, { x: 124, y: 306 }, bounds, widget)
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
