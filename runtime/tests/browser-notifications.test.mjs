import assert from 'node:assert/strict'
import test from 'node:test'
import {
  getBrowserNotificationPermission,
  requestBrowserNotificationPermission,
  resetBrowserNotificationRegistrationForTests,
  showBrowserSystemNotification,
} from '../../src/lib/browser-notifications.ts'

test('browser notifications use the service worker system-notification API when available', async () => {
  resetBrowserNotificationRegistrationForTests()
  const shown = []
  const registration = {
    async showNotification(title, options) {
      shown.push({ title, options })
    },
  }
  let registered
  class FakeNotification {
    static permission = 'granted'
    constructor() {
      throw new Error('window Notification fallback should not run')
    }
  }
  const windowRef = {
    Notification: FakeNotification,
    isSecureContext: true,
    location: { href: 'http://127.0.0.1:5180/#/chat' },
  }
  const navigatorRef = {
    serviceWorker: {
      async register(path, options) {
        registered = { path, options }
        return registration
      },
      ready: Promise.resolve(registration),
    },
  }

  assert.deepEqual(
    await showBrowserSystemNotification(
      {
        title: 'Pisper test',
        body: 'Completed',
        tag: 'pisper-test',
      },
      { windowRef, navigatorRef },
    ),
    { shown: true, transport: 'service-worker' },
  )
  assert.deepEqual(registered, { path: '/notification-sw.js', options: { scope: '/' } })
  assert.deepEqual(shown, [
    {
      title: 'Pisper test',
      options: {
        body: 'Completed',
        tag: 'pisper-test',
        data: { url: 'http://127.0.0.1:5180/#/chat' },
      },
    },
  ])
})

test('browser notifications fall back to the page Notification API only without service workers', async () => {
  resetBrowserNotificationRegistrationForTests()
  let created
  class FakeNotification {
    static permission = 'granted'
    constructor(title, options) {
      created = { title, options, closed: false }
      return created
    }
  }
  const windowRef = {
    Notification: FakeNotification,
    isSecureContext: true,
    location: { href: 'http://localhost/chat' },
    focus() {},
  }

  assert.deepEqual(
    await showBrowserSystemNotification(
      { title: 'Fallback', body: 'Body' },
      {
        windowRef,
        navigatorRef: {},
      },
    ),
    { shown: true, transport: 'window' },
  )
  assert.equal(created.title, 'Fallback')
  assert.equal(created.options.body, 'Body')
})

test('browser notification permission helpers preserve granted, denied, and requested states', async () => {
  class GrantedNotification {
    static permission = 'granted'
  }
  assert.equal(
    getBrowserNotificationPermission({ windowRef: { Notification: GrantedNotification } }),
    'granted',
  )
  assert.equal(getBrowserNotificationPermission({ windowRef: {} }), 'unsupported')

  class DefaultNotification {
    static permission = 'default'
    static async requestPermission() {
      return 'granted'
    }
  }
  assert.equal(
    await requestBrowserNotificationPermission({
      windowRef: { Notification: DefaultNotification },
    }),
    'granted',
  )
})
