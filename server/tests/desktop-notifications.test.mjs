import assert from 'node:assert/strict'
import test from 'node:test'
import {
  getDesktopNotificationStatus,
  parseRegistryDword,
  windowsNotificationStatus,
} from '../../electron/desktop-notifications.mjs'

test('Windows notification registry DWORD values are parsed from reg.exe output', () => {
  const output = `\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\PushNotifications\n    ToastEnabled    REG_DWORD    0x0\n`
  assert.equal(parseRegistryDword(output, 'ToastEnabled'), 0)
  assert.equal(parseRegistryDword(output.replace('0x0', '0x1'), 'ToastEnabled'), 1)
  assert.equal(parseRegistryDword(output, 'MissingValue'), null)
})

test('Windows reports global and per-app notification blocks instead of assuming permission', () => {
  assert.deepEqual(windowsNotificationStatus({ toastEnabled: 0, globalToastsEnabled: 1, appEnabled: 1 }), {
    supported: true,
    permission: 'denied',
    reason: 'system-disabled',
  })
  assert.deepEqual(windowsNotificationStatus({ toastEnabled: 1, globalToastsEnabled: 1, appEnabled: 0 }), {
    supported: true,
    permission: 'denied',
    reason: 'app-disabled',
  })
  assert.equal(windowsNotificationStatus({ toastEnabled: null, globalToastsEnabled: null, appEnabled: null }).permission, 'granted')
})

test('desktop notification status queries Windows settings and handles unsupported platforms', async () => {
  const values = new Map([
    ['ToastEnabled', 1],
    ['NOC_GLOBAL_SETTING_TOASTS_ENABLED', 0],
    ['Enabled', 1],
  ])
  const status = await getDesktopNotificationStatus({
    platform: 'win32',
    appUserModelId: 'com.lingkongran.pisper',
    queryDword: async (_key, valueName) => values.get(valueName) ?? null,
  })
  assert.equal(status.permission, 'denied')
  assert.equal(status.reason, 'system-disabled')
  assert.deepEqual(await getDesktopNotificationStatus({ platform: 'linux', isSupported: false }), {
    supported: false,
    permission: 'unsupported',
    reason: 'unsupported',
  })
})
