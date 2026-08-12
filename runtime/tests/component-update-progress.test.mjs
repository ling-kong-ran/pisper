import assert from 'node:assert/strict'
import test from 'node:test'
import {
  componentUpdateStatus,
  currentDesktopVersion,
} from '../../src/features/updates/component-update-state.ts'

function component(component, state, size, transferred = 0) {
  return {
    component,
    state,
    currentVersion: '0.4.30',
    availableVersion: state === 'current' ? '0.4.30' : '0.4.31',
    message: state === 'error' ? 'network interrupted' : '',
    releaseUrl: 'https://example.test/release',
    notes: '',
    size,
    transferred,
    canInstall: state === 'available' || state === 'error',
    restartRequired: state === 'installed',
  }
}

test('desktop product version follows the active frontend component instead of the host shell', () => {
  const items = [component('desktop', 'current', 0)]
  items[0].currentVersion = '0.5.1'

  assert.equal(currentDesktopVersion('0.4.48', items), '0.5.1')
  assert.equal(currentDesktopVersion('0.4.48'), '0.4.48')
  assert.equal(currentDesktopVersion('0.4.48', []), '0.4.48')
})

test('component update progress aggregates completed, active, and pending component bytes', () => {
  const status = componentUpdateStatus(
    [
      component('desktop', 'installed', 20, 20),
      component('tui', 'downloading', 30, 15),
      component('runtime', 'available', 50),
    ],
    '2026-08-09T00:00:00.000Z',
  )

  assert.equal(status.state, 'downloading')
  assert.equal(status.total, 100)
  assert.equal(status.transferred, 35)
  assert.equal(status.percent, 35)
})

test('remaining component downloads keep the batch active after another component fails', () => {
  const status = componentUpdateStatus([
    component('desktop', 'error', 20),
    component('tui', 'downloading', 30, 6),
    component('runtime', 'available', 50),
  ])

  assert.equal(status.state, 'downloading')
  assert.equal(status.total, 100)
  assert.equal(status.transferred, 6)
  assert.equal(status.percent, 6)
  assert.match(status.message, /desktop: network interrupted/)
})
