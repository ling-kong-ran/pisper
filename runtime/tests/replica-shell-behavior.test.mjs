import assert from 'node:assert/strict'
import test from 'node:test'
import { clampSidebarWidth, DEFAULT_SIDEBAR_WIDTH } from '../../src/lib/sidebar-width.ts'
import { uiTextSizes } from '../../src/app/ui-preferences.ts'

test('sidebar restores a valid preference while reserving space for the conversation', () => {
  assert.equal(DEFAULT_SIDEBAR_WIDTH, 340)
  assert.equal(clampSidebarWidth(undefined, 1200), 340)
  assert.equal(clampSidebarWidth(NaN, 1200), 340)
  assert.equal(clampSidebarWidth(Infinity, 1200), 340)
  assert.equal(clampSidebarWidth(100, 1200), 240)
  assert.equal(clampSidebarWidth(1000, 1200), 520)
  assert.equal(clampSidebarWidth(480, 650), 410)
  assert.equal(clampSidebarWidth(480, 400), 240)
  // 窄窗只约束当前渲染；恢复宽窗时仍使用用户保存的偏好。
  const preferred = 480
  assert.equal(clampSidebarWidth(preferred, 650), 410)
  assert.equal(clampSidebarWidth(preferred, 1200), preferred)
})

test('UI font preferences round their scale without changing content and code font roles', () => {
  // 规格 6.4 字号阶梯：元信息 12 / 密集控件 13 / 卡片标题 14 / 面板 h1 16 / 桌面页 h1 20 /
  // 手机 h1 24 / 数字主值 28；大号收在 28 以内，小号与大号按 14px 基准等比取整。
  assert.deepEqual(uiTextSizes('default'), [12, 13, 14, 16, 20, 24, 28])
  assert.deepEqual(uiTextSizes('small'), [12, 13, 14, 15, 19, 23, 27])
  assert.deepEqual(uiTextSizes('large'), [12, 13, 15, 17, 21, 25, 29])
  assert.deepEqual(uiTextSizes('retired-preference'), uiTextSizes('default'))
})
