// Bot 的对话时间线：派出的子 agent 按开始时间插进对话，作为事件条目；它们不算“正在写的回复”。
// 子 agent 摘要按不可信数据解析；侧栏的一句话写出在做几件、正在做哪件。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

import { lastSettledMessageIndex } from '../../src/lib/session-state.ts'
import { botActivityLine } from '../../src/features/bots/bot-presentation.ts'
import {
  isActiveAgent,
  parseSessionAgents,
  parseSessionLinks,
  withAgentEvents,
} from '../../src/features/chat/session-agents.ts'

const agent = (id, status, startedAt, extra = {}) => ({
  id,
  kind: 'agent',
  taskName: id,
  status,
  message: '',
  model: '',
  output: '',
  error: '',
  startedAt,
  completedAt: null,
  toolCallCount: 0,
  activity: '',
  ...extra,
})

test('jobs are placed in the timeline by start time and never count as the reply being written', () => {
  const messages = [
    { id: 'u1', role: 'user', text: '修首页', timestamp: Date.parse('2026-11-04T10:00:00Z') },
    { id: 'a1', role: 'agent', text: '收到', timestamp: Date.parse('2026-11-04T10:00:02Z') },
    // 第一件活做完后 Bot 的汇报（有时间）；之后刚发出、还没落盘的两条没有时间。
    { id: 'r1', role: 'agent', text: '做完了', timestamp: Date.parse('2026-11-04T10:01:00Z') },
    { id: 'u2', role: 'user', text: '顺手部署' },
    { id: 'a2', role: 'agent', text: '收到', streaming: true },
  ]
  const merged = withAgentEvents(messages, [
    agent('job2', 'running', '2026-11-04T10:05:00Z'),
    agent('job1', 'completed', '2026-11-04T10:00:03Z'),
  ])
  assert.deepEqual(
    merged.map((item) => item.id),
    ['u1', 'a1', 'agent-event-job1', 'r1', 'u2', 'a2', 'agent-event-job2'],
  )
  // 末尾的事件条目不是回复：正在写的仍是 a2。
  assert.equal(merged[lastSettledMessageIndex(merged)].id, 'a2')
  assert.equal(withAgentEvents(messages, []), messages)
  // Bot 之间的来往也是事件条目，按开始时间插进去。
  const withLink = withAgentEvents(
    messages.slice(0, 2),
    [],
    [
      {
        id: 'ch_abcdefghij',
        botId: 'bot_bbbbbbbbbbbb',
        direction: 'out',
        text: '帮我跑一遍回归',
        status: 'waiting',
        reason: '',
        openedAt: '2026-11-04T10:00:01Z',
      },
    ],
  )
  assert.deepEqual(
    withLink.map((item) => item.id),
    ['u1', 'link-event-ch_abcdefghij', 'a1'],
  )
})

test('bot links are parsed as untrusted data', () => {
  assert.deepEqual(
    parseSessionLinks({
      links: [
        {
          id: 'ch_abcdefghij',
          botId: 'bot_bbbbbbbbbbbb',
          direction: 'out',
          text: '帮我跑一遍回归',
          status: 'answered',
          openedAt: '2026-11-04T10:00:01Z',
        },
        { id: 'ch_x', botId: 'bot_b', status: 'weird' },
        { botId: 'bot_b', status: 'waiting' },
      ],
    }),
    [
      {
        id: 'ch_abcdefghij',
        botId: 'bot_bbbbbbbbbbbb',
        direction: 'out',
        text: '帮我跑一遍回归',
        status: 'answered',
        reason: '',
        openedAt: '2026-11-04T10:00:01Z',
      },
    ],
  )
  assert.deepEqual(parseSessionLinks({}), [])
})

test('sub-agent summaries are parsed as untrusted data, titles first', () => {
  const agents = parseSessionAgents({
    agents: [
      {
        id: 'a1',
        taskName: 'safari_16',
        title: '修首页 Safari 16 白屏',
        status: 'running',
        currentActivity: { name: 'bash' },
        toolCallCount: 3,
        startedAt: '2026-11-04T10:00:00Z',
      },
      { id: 'a2', taskName: 'x', status: 'exploded' },
      { taskName: 'no id', status: 'running' },
      'junk',
    ],
  })
  assert.equal(agents.length, 1)
  assert.equal(agents[0].taskName, '修首页 Safari 16 白屏')
  assert.equal(agents[0].activity, 'bash')
  assert.equal(agents[0].toolCallCount, 3)
  assert.ok(isActiveAgent(agents[0]))
  assert.deepEqual(parseSessionAgents(null), [])
})

test('the sidebar line says how many jobs are going and which one is current', () => {
  const t = (key, values = {}) =>
    ({
      'bots:activity.workingOn': `${values.count} 件在做 · 在做：${values.task}`,
      'bots:activity.working': `${values.count} 件在做`,
      'bots:activity.approval': '等你批准一个操作',
      'bots:activity.idle': '空闲',
      'bots:activity.paused': '已暂停',
      'bots:activity.busy': '在忙',
      'bots:activity.question': '有问题等你回答',
    })[key] ?? key
  const activity = (extra) => ({
    status: 'running',
    waiting: null,
    needsYou: false,
    working: 0,
    current: '',
    ...extra,
  })
  assert.equal(
    botActivityLine(t, { enabled: true, activity: activity({ working: 3, current: '修首页' }) }),
    '3 件在做 · 在做：修首页',
  )
  assert.equal(
    botActivityLine(t, {
      enabled: true,
      activity: activity({ waiting: 'approval', needsYou: true, working: 1 }),
    }),
    '等你批准一个操作 · 1 件在做',
  )
  assert.equal(botActivityLine(t, { enabled: true, activity: activity({}) }), '在忙')
})

test('the bot conversation page shows jobs from the session and reloads after they finish', async () => {
  const focus = await readFile(
    new URL('../../src/features/chat/FocusSession.tsx', import.meta.url),
    'utf8',
  )
  assert.match(
    focus,
    /return botId \? withAgentEvents\(shown, sessionAgents\.agents, sessionAgents\.links\) : shown/,
  )
  assert.match(focus, /useSessionAgents\(session\.id, Boolean\(botId\), \(\) => \{/)
  const transcript = await readFile(
    new URL('../../src/features/chat/VirtualMessageTranscript.tsx', import.meta.url),
    'utf8',
  )
  assert.match(transcript, /if \(message\.role === 'event'\) \{/)
})
