// 提示音：提醒时刻（需要你、跑完、失败）从看板卡片前后两次快照推出；偏好按不可信输入解析；
// 键登记在浏览器偏好白名单里；应用壳只在开启时、按设置的音效响一声。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

import { BROWSER_PREFERENCE_KEYS } from '../../shared/browser-preferences.mjs'
import { STORAGE_KEYS } from '../../src/app/storage.ts'
import { sessionAlerts } from '../../src/features/board/session-alerts.ts'
import { NOTIFICATION_SOUNDS, isNotificationSound } from '../../src/lib/notification-sounds.ts'
import {
  DEFAULT_NOTIFICATION_SOUND_PREFERENCE,
  parseNotificationSoundPreference,
} from '../../src/stores/notification-sound-store.ts'

const item = (id, lane, status, extra = {}) => ({
  id,
  title: `会话 ${id}`,
  lane,
  status,
  since: null,
  startedAt: null,
  pinned: false,
  unread: false,
  approvals: [],
  approvalsState: 'ready',
  severity: null,
  bot: null,
  ...extra,
})

test('alerts come from changes between snapshots, never from the first load', () => {
  const first = sessionAlerts(null, [
    item('a', 'running', 'running'),
    item('b', 'attention', 'approval'),
    item('c', 'done', 'done'),
  ])
  assert.deepEqual(first.alerts, [])
  const second = sessionAlerts(first.snapshot, [
    item('a', 'done', 'done'),
    item('b', 'attention', 'approval'),
    item('c', 'done', 'done'),
    item('d', 'attention', 'question'),
  ])
  assert.deepEqual(
    second.alerts.map((alert) => [alert.kind, alert.id]),
    [
      ['finished', 'a'],
      ['waiting', 'd'],
    ],
  )
  // 在跑的会话失败；只在等你的会话被批准后继续跑不算提醒。
  const third = sessionAlerts(second.snapshot, [
    item('a', 'done', 'done'),
    item('b', 'running', 'running'),
    item('d', 'attention', 'failed'),
  ])
  assert.deepEqual(
    third.alerts.map((alert) => [alert.kind, alert.id]),
    [['failed', 'd']],
  )
  // 旧的失败会话重新进入列表（时间窗、筛选变化）不报。
  assert.deepEqual(sessionAlerts(third.snapshot, [item('old', 'attention', 'failed')]).alerts, [])
})

test('sound preferences parse untrusted storage and register their key', () => {
  assert.equal(NOTIFICATION_SOUNDS.length, 5)
  assert.ok(NOTIFICATION_SOUNDS.every(isNotificationSound))
  assert.deepEqual(
    parseNotificationSoundPreference('not json'),
    parseNotificationSoundPreference(null),
  )
  assert.deepEqual(parseNotificationSoundPreference(null).sounds, {
    ...DEFAULT_NOTIFICATION_SOUND_PREFERENCE.sounds,
  })
  const parsed = parseNotificationSoundPreference(
    JSON.stringify({
      enabled: false,
      volume: 250,
      whileFocused: false,
      sounds: { waiting: 'bell', finished: 'none', failed: 'siren' },
    }),
  )
  assert.equal(parsed.enabled, false)
  assert.equal(parsed.volume, 100)
  assert.equal(parsed.whileFocused, false)
  assert.deepEqual(parsed.sounds, { waiting: 'bell', finished: 'none', failed: 'low' })
  assert.equal(STORAGE_KEYS.notificationSounds, 'pisper-notification-sounds')
  assert.ok(BROWSER_PREFERENCE_KEYS.includes('pisper-notification-sounds'))
})

test('the app shell plays one sound per batch, respecting the switches', async () => {
  const app = await readFile(new URL('../../src/App.tsx', import.meta.url), 'utf8')
  assert.match(app, /useEffect\(\(\) => installNotificationSoundUnlock\(\), \[\]\)/)
  assert.match(app, /if \(!preference\.enabled\) return/)
  assert.match(app, /\(\['waiting', 'failed', 'finished'\] as const\)\.find/)
  assert.match(app, /onAlerts=\{playAlertSound\}/)
  const settings = await readFile(
    new URL('../../src/features/config/NotificationSettings.tsx', import.meta.url),
    'utf8',
  )
  assert.match(settings, /<NotificationSoundSettings \/>/)
})
