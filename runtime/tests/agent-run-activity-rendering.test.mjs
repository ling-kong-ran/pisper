import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import AgentRunActivity from '../../src/features/chat/AgentRunActivity.tsx'
import { translateText } from '../../src/app/i18n.ts'

const t = (key, values) => translateText(key, 'zh-CN', values)
const render = (props) => renderToStaticMarkup(React.createElement(AgentRunActivity, props))

test('retry notices show progress while keeping provider diagnostics collapsed', () => {
  const rawError = '429 rate limit: long provider response'
  const summary = t('chat:requestNotice.retryingAttempt', { attempt: 1, maxAttempts: 3 })
  for (const activity of [
    { type: 'retry', message: rawError, summary },
    // 旧历史没有摘要，仍使用简短标题，详情按需展开。
    { type: 'retry', message: rawError },
  ]) {
    const html = render({ streaming: true, activityFeed: [activity] })
    assert.ok(html.includes(activity.summary || t('chat:agentRunActivity.retryingRequest')))
    assert.match(html, /aria-expanded="false"/)
    assert.doesNotMatch(html, /429 rate limit|long provider response/)
  }
})

test('agent activity renders task names without canonical paths or sequence IDs', () => {
  for (const status of ['queued', 'running', 'completed', 'interrupted', 'failed']) {
    const agent = Object.freeze({
      id: 'agent-250',
      canonicalName: '/root/speech-terms-ui_250',
      taskName: 'speech-terms-ui',
      status,
    })
    const html = render({ activityFeed: [{ type: 'agent', agent }] })
    assert.match(html, /speech-terms-ui/)
    assert.doesNotMatch(html, /\/root\/|speech-terms-ui_250|agent-250/)
    assert.equal(agent.canonicalName, '/root/speech-terms-ui_250')
  }
})

test('missing task names use the friendly translated fallback', () => {
  for (const taskName of [undefined, '', '  ']) {
    const html = render({
      activityFeed: [
        {
          type: 'agent',
          agent: { canonicalName: '/root/speech-terms-ui_250', taskName, status: 'running' },
        },
      ],
    })
    assert.ok(html.includes(t('chat:agentRunActivity.subagent')))
    assert.doesNotMatch(html, /\/root\/|speech-terms-ui_250/)
  }
})

test('cleared team snapshots remove the team panel and progress without relabeling paused history', () => {
  const team = {
    id: 'team-1',
    status: 'paused',
    taskCount: 2,
    completedTaskCount: 1,
    tasks: [{ id: 'task-1', taskName: 'Previous task', status: 'completed' }],
  }
  const props = { streaming: true, team }
  const history = render(props)
  assert.ok(history.includes(t('chat:agentRunActivity.teamPaused')))
  assert.match(history, /1\/2/)
  const cleared = render({ ...props, team: null })
  assert.doesNotMatch(cleared, /agent-team-panel|1\/2|Previous task/)
  assert.ok(!cleared.includes(t('chat:agentRunActivity.teamPaused')))
  assert.ok(!cleared.includes(t('chat:agentRunActivity.teamActive')))
  assert.equal(team.status, 'paused')
})

test('the computer use tool shows what it is doing and its screenshot', () => {
  const html = render({
    activityFeed: [
      {
        type: 'tool',
        id: 'tool-c',
        name: 'computer_use',
        args: { action: 'key', keys: 'ctrl+s' },
        status: 'running',
        previewImage: {
          id: 'a',
          url: '/api/assets/a/download?inline=1',
          name: 'a.jpg',
          mimeType: 'image/jpeg',
        },
      },
    ],
  })
  assert.ok(html.includes(t('chat:agentRunActivity.operatingComputer')))
  assert.ok(html.includes('ctrl+s'))
  assert.ok(html.includes('/api/assets/a/download?inline=1'))
})

// 旧版会话的历史记录里还有这些工具名，要能正常显示。
test('computer use tool activities render localized labels and live preview images', () => {
  const previewImage = {
    id: 'asset-1',
    url: '/api/assets/asset-1/download?inline=1',
    name: 'observe_ui-2026-09-15T000000.000Z.png',
    mimeType: 'image/png',
  }
  const html = render({
    activityFeed: [
      {
        type: 'tool',
        id: 'tool-0',
        name: 'observe_ui',
        args: { mode: 'visual' },
        status: 'running',
        previewImage,
      },
    ],
  })
  // 工具活动使用 computer use 专属文案，避免退回通用工具标签。
  assert.ok(html.includes(t('chat:agentRunActivity.observingInterface')))
  // 实时窗口预览图：锚点 + 懒加载图像都指向资产内联下载地址。
  assert.ok(html.includes('agent-run-tool-preview'))
  assert.ok(html.includes(previewImage.url))
  assert.match(html, /loading="lazy"/)
  // act_ui 完成态使用专属完成文案。
  const doneHtml = render({
    activityFeed: [{ type: 'tool', id: 'tool-1', name: 'act_ui', args: {}, status: 'done' }],
  })
  assert.ok(doneHtml.includes(t('chat:agentRunActivity.interfaceActionCompleted')))
})
