// 输入框工具行与托盘（规格 v2 第 4.3 节）。原来的可配置工具条（composer-toolbar-layout /
// composer-toolbar-store）退役：工具行固定为 [+] [盾牌芯片] … [模型芯片] [发送]，运行状态与排队消息
// 移到输入框上方的托盘。这里保护：退役偏好不破坏旧快照恢复、托盘的内容与触控尺寸、发送键形状。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { i18n } from '../../src/app/i18n.ts'
import { boardMessages } from '../../src/app/board-messages.ts'
import {
  BROWSER_PREFERENCE_KEYS,
  parseBrowserPreferenceSnapshot,
} from '../../shared/browser-preferences.mjs'
import {
  ComposerSendButton,
  ComposerTray,
} from '../../src/features/chat/focus-session-composer-bits.tsx'

i18n.addResourceBundle('zh-CN', 'board', boardMessages.zh)
i18n.addResourceBundle('en-US', 'board', boardMessages.en)

const render = (type, props) => renderToStaticMarkup(React.createElement(type, props))

test('retired toolbar preferences still restore from old snapshots without any reader left', async () => {
  // 旧快照带着这两个键：白名单里删掉它们会让整份快照解析失败，其他偏好也恢复不了。
  for (const key of ['pisper-composer-toolbar', 'pisper-zcode-composer-toolbar'])
    assert.ok(BROWSER_PREFERENCE_KEYS.includes(key), key)
  const snapshot = parseBrowserPreferenceSnapshot({
    version: 1,
    values: {
      'pisper-composer-toolbar': JSON.stringify({ state: { layout: { inline: ['model'] } } }),
      'pisper-ui': '{}',
    },
  })
  assert.equal(snapshot.values['pisper-ui'], '{}')
  const sources = await Promise.all(
    ['FocusSession.tsx', 'focus-session-composer-bits.tsx', 'ComposerModeChip.tsx'].map((file) =>
      readFile(`src/features/chat/${file}`, 'utf8'),
    ),
  )
  for (const source of sources) assert.doesNotMatch(source, /pisper-composer-toolbar/)
})

test('the tray shows only “running 3:12 · stop Esc” and never repeats the running command', () => {
  const html = render(ComposerTray, {
    streaming: true,
    startedAt: new Date(Date.now() - 192_000).toISOString(),
    onStop() {},
  })
  assert.match(html, /data-run-bar="running"/)
  assert.match(html, /运行中/)
  assert.match(html, /data-tray-clock[^>]*>3:1\d</)
  assert.match(html, /data-run-stop/)
  assert.match(html, /<kbd[^>]*>Esc<\/kbd>/)
  // 托盘不接收当前活动：没有命令、工具名或用量读数。
  assert.doesNotMatch(html, /npm|bash|上下文|tok|缓存/)
  // 手机：停止键 72×44。
  assert.match(html, /max-\[650px\]:h-11 max-\[650px\]:min-w-\[72px\]/)
  // 没有运行、压缩与排队时托盘整个不出现。
  assert.equal(render(ComposerTray, { streaming: false, queuedInputs: [] }), '')
})

test('queued messages are one row each with edit and cancel revealed on hover, 44px on touch', () => {
  const html = render(ComposerTray, {
    streaming: true,
    onStop() {},
    queuedInputs: [
      { id: 'q1', text: '顺便把 README 里的登录流程说明也更新一下', behavior: 'followUp' },
      { id: 'q2', text: '先跑单测', behavior: 'steer' },
    ],
    withdrawingInputIds: ['q2'],
    onEdit() {},
    onCancel() {},
  })
  assert.match(html, /data-pisper-queue-size="2"/)
  assert.equal((html.match(/<li\b/g) || []).length, 2)
  assert.match(html, /data-queued-behavior="followUp"[^>]*>排队</)
  assert.match(html, /data-queued-behavior="steer"[^>]*>插话</)
  assert.equal((html.match(/data-queued-edit/g) || []).length, 2)
  assert.equal((html.match(/data-queued-cancel/g) || []).length, 2)
  // 桌面悬停 / 聚焦才出现，手机与触屏常驻 44×44。
  assert.match(
    html,
    /opacity-0[^"]*group-hover\/queued:opacity-100[^"]*group-focus-within\/queued:opacity-100/,
  )
  assert.match(html, /max-\[650px\]:size-11 max-\[650px\]:opacity-100/)
  // 正在撤回的那条两个键都禁用，并标记忙。
  assert.match(html, /<li[^>]*aria-busy="true"/)
  assert.equal((html.match(/ disabled=""/g) || []).length, 2)
})

test('the send key is a graphite circle that never doubles as stop', () => {
  const html = render(ComposerSendButton, { queueing: false, disabled: false })
  assert.match(html, /send-button[^"]*rounded-full[^"]*bg-star/)
  assert.doesNotMatch(html, /data-run-stop|lucide-square/)
})
