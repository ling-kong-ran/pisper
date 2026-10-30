// 第 3 期会话页内容（规格 4.3 / 4.4 / 3.5）：工具组与运行中 bash 行、代码块文件名、运行条读数、
// ↵ 排队 · ⌥↵ 插话、Esc 只停止不返回、会话审批的完整命令与 critical 确认，以及改动标签的统一差异。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { i18n } from '../../src/app/i18n.ts'
import { boardMessages } from '../../src/app/board-messages.ts'
import AgentRunActivity from '../../src/features/chat/AgentRunActivity.tsx'
import { steerBindingFor } from '../../src/features/chat/composer-run-input.ts'
import { ComposerStatusPill } from '../../src/features/chat/focus-session-composer-bits.tsx'
import {
  intralineRange,
  parseUnifiedDiff,
  unifiedDiffLines,
} from '../../src/features/chat/git-diff.ts'
import { ToolApproval } from '../../src/features/chat/ToolApproval.tsx'
import { codeBlockFileName } from '../../src/lib/code-block-meta.ts'

const root = resolve(import.meta.dirname, '../..')
const read = (path) => readFile(resolve(root, path), 'utf8')

// 会话路由会随页注册看板命名空间；测试里同样注册，断言真实文案而不是键名。
i18n.addResourceBundle('zh-CN', 'board', boardMessages.zh)
i18n.addResourceBundle('en-US', 'board', boardMessages.en)

const render = (type, props) => renderToStaticMarkup(React.createElement(type, props))

test('the tool group header counts operations and the running bash row opens in run cyan', () => {
  const html = render(AgentRunActivity, {
    streaming: true,
    startedAt: new Date(Date.now() - 12_000).toISOString(),
    activityFeed: [
      { type: 'tool', id: 't1', name: 'read', args: { path: 'src/auth/login.ts' }, status: 'done' },
      { type: 'tool', id: 't2', name: 'edit', args: { path: 'src/auth/login.ts' }, status: 'done' },
      {
        type: 'tool',
        id: 't3',
        name: 'bash',
        args: { command: 'npm test -- --runInBand' },
        status: 'running',
        startedAt: new Date(Date.now() - 4_000).toISOString(),
        output:
          'PASS src/a.test.ts\n✗ login › rejects expired token\n✓ login › accepts valid token',
      },
    ],
  })
  assert.match(html, /data-tool-group-summary/)
  for (const text of ['3 个操作', '2 完成', '1 运行中']) assert.ok(html.includes(text), text)
  assert.doesNotMatch(html, /失败<\/span>/)
  // 运行中的 bash 行默认展开，行用运行青浅底，输出里失败行标红、通过行标绿。
  assert.match(html, /<details[^>]*agent-run-command-output[^>]*data-running="true"[^>]*open/)
  assert.match(html, /<summary[^>]*bg-st-run-soft/)
  assert.match(html, /class="text-st-fail">✗ login/)
  assert.match(html, /class="text-st-done">✓ login/)
  assert.ok(html.includes('npm test -- --runInBand'))
  // 状态字形来自共用的 StatusGlyph（形状 + 状态色），运行中用弧形。
  assert.match(html, /data-status-glyph="run"/)
  assert.match(html, /data-status-glyph="done"/)

  // 结束后没有“运行中”：残留的 running 记录按完成计，失败单独计数。
  const done = render(AgentRunActivity, {
    streaming: false,
    activityFeed: [
      { type: 'tool', id: 't1', name: 'bash', args: { command: 'npm test' }, status: 'error' },
      { type: 'tool', id: 't2', name: 'read', args: { path: 'a.ts' }, status: 'running' },
    ],
  })
  assert.ok(done.includes('2 个操作'))
  assert.ok(done.includes('1 失败'))
  assert.ok(!done.includes('运行中'))
})

test('reasoning collapses into a chip that stays closed by default', () => {
  const html = render(AgentRunActivity, {
    streaming: false,
    thinkingText: '先校验签名，再判断过期。',
  })
  assert.match(html, /<details class="agent-thinking-window[^"]*completed"/)
  assert.doesNotMatch(html, /<details class="agent-thinking-window[^"]*"[^>]*\sopen/)
  assert.match(html, /agent-thinking-head[^"]*ring-1/)
})

test('code block file names come from the fence info and are clipped display text', () => {
  assert.equal(codeBlockFileName('language-ts', 'title="src/auth/login.ts"'), 'src/auth/login.ts')
  assert.equal(codeBlockFileName('language-ts:src/a.ts', ''), 'src/a.ts')
  assert.equal(codeBlockFileName('language-bash', 'scripts/run.sh {1,3}'), 'scripts/run.sh')
  assert.equal(codeBlockFileName('language-ts', ''), '')
  assert.equal(codeBlockFileName('language-ts', 'showLineNumbers'), '')
  assert.equal(codeBlockFileName('language-ts', `file=${'a/'.repeat(200)}x.ts`).length, 160)
  assert.equal(codeBlockFileName('language-ts', 'title="a\u0007b.ts"'), 'ab.ts')
})

test('running input queues with the send key and steers with Alt added to it', () => {
  assert.equal(steerBindingFor('Enter'), 'Alt+Enter')
  assert.equal(steerBindingFor('Mod+Enter'), 'Mod+Alt+Enter')
  assert.equal(steerBindingFor('Mod+Shift+Enter'), 'Mod+Alt+Shift+Enter')
  assert.equal(steerBindingFor('Alt+Enter'), null)
  assert.equal(steerBindingFor(''), null)
})

test('session composer wiring: ↵ follows up, ⌥↵ steers, Esc stops without navigating', async () => {
  const [focus, composer] = await Promise.all([
    read('src/features/chat/FocusSession.tsx'),
    read('src/features/chat/use-focus-composer.ts'),
  ])
  // 运行中提交走 /input，方式由输入框决定；会话页默认排队，只有 ⌥↵ 记成插话。
  assert.match(composer, /onQueue\?\.\(value, selection\.attachments, runningBehavior, accept\)/)
  assert.match(focus, /useRef<'followUp' \| 'steer'>\('followUp'\)/)
  assert.match(focus, /runningBehaviorRef\.current = 'steer'/)
  assert.match(focus, /void submit\(event, pageChrome \? behavior : 'steer'\)/)
  // Esc：捕获阶段记下浮层，冒泡阶段才停止；处理函数里没有任何路由跳转。
  const escape = focus.slice(
    focus.indexOf('const stopRef = useRef(stop)'),
    focus.indexOf('// 输入框工具条与会话页头'),
  )
  assert.match(escape, /window\.addEventListener\('keydown', capture, true\)/)
  assert.match(escape, /escapeLayerOpen\(\)/)
  assert.match(escape, /void stopRef\.current\(\)/)
  assert.doesNotMatch(escape, /navigate|history\.|backToBoard/)
  // 发送键仍只负责发送，运行条是唯一停止入口。
  assert.doesNotMatch(focus, /<ComposerSendButton[^>]*onStop/)
})

test('the run bar shows only the readings it actually has and a transform-only progress line', () => {
  const html = render(ComposerStatusPill, {
    streaming: true,
    startedAt: new Date(Date.now() - 12_000).toISOString(),
    activity: { type: 'tool', name: 'bash', status: 'running', args: { command: 'npm test' } },
    onStop: () => {},
    metrics: { contextPercent: 38.4, tokens: 42_100, cacheHitRate: 71, queued: 1 },
  })
  for (const text of ['上下文 38%', '缓存命中 71%', '排队 1 条', 'npm test'])
    assert.ok(html.includes(text), text)
  assert.match(html, /data-run-bar="running"/)
  assert.match(html, /data-run-stop/)
  assert.match(html, /animate-\[hairline-sweep[^"]*bg-st-run-fill/)
  const partial = render(ComposerStatusPill, {
    streaming: true,
    onStop: () => {},
    metrics: { contextPercent: null, tokens: null, cacheHitRate: null, queued: 0 },
  })
  assert.doesNotMatch(partial, /data-run-metrics|上下文|缓存命中/)
  assert.match(render(ComposerStatusPill, { streaming: false }), /idle/)
})

// 审批卡读取手机外壳判断（matchMedia）；服务端渲染时给一个桌面宽度的最小 window。
function withDesktopWindow(run) {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: {
      matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    },
  })
  try {
    return run()
  } finally {
    if (previous) Object.defineProperty(globalThis, 'window', previous)
    else delete globalThis.window
  }
}

test('session approvals show the whole command and keep critical approvals behind the confirm layer', () =>
  withDesktopWindow(() => {
    const longCommand =
      `git push --force-with-lease origin fix/ci ${'--verbose '.repeat(40)}`.trim()
    const standard = render(ToolApproval, {
      approvals: [
        { id: 'a1', toolName: 'bash', args: { command: 'npm test -- --runInBand' }, reason: '' },
      ],
      onResolve: () => {},
    })
    assert.match(standard, /data-approval-command/)
    assert.ok(standard.includes('npm test -- --runInBand'))
    assert.match(standard, /data-severity="standard"/)
    assert.doesNotMatch(standard, /data-approval-critical/)
    const critical = render(ToolApproval, {
      approvals: [
        {
          id: 'a2',
          toolName: 'bash',
          args: { command: longCommand },
          reason: '命令守卫拦截：改写远端分支。需要确认。',
        },
      ],
      onResolve: () => {},
    })
    // 完整命令原样在卡片上（不截断，只按行折叠），危险等级写出守卫原因，“允许…”是默认样式。
    assert.ok(critical.includes(longCommand))
    assert.match(critical, /data-severity="critical"/)
    assert.ok(critical.includes('高风险 · 改写远端分支'))
    assert.match(
      critical,
      /data-approval-critical="true"[^>]*aria-haspopup="dialog"|aria-haspopup="dialog"[^>]*data-approval-critical="true"/,
    )
    assert.match(
      critical,
      /data-variant="outline"[^>]*data-approval-critical|data-approval-critical[^>]*data-variant="outline"/,
    )
  }))

test('unified diff lists deletions before additions and marks the changed words', () => {
  const diff = [
    'diff --git a/src/auth/login.ts b/src/auth/login.ts',
    '--- a/src/auth/login.ts',
    '+++ b/src/auth/login.ts',
    '@@ -14,3 +14,4 @@ export async function handleLogin',
    '   const token = readBearer(req)',
    '-  const payload = decode(token)',
    '+  const payload = verifyToken(token)',
    '+  assertTokenFresh(payload)',
    '   return ok()',
  ].join('\n')
  const lines = unifiedDiffLines(parseUnifiedDiff(diff)[0])
  assert.deepEqual(
    lines.map((line) =>
      line.kind === 'line' ? `${line.tone}:${line.oldLine}:${line.newLine}` : line.kind,
    ),
    [
      'header',
      'context:14:14',
      'deleted:15:null',
      'added:null:15',
      'added:null:16',
      'context:16:17',
    ],
  )
  const deleted = lines[2]
  const added = lines[3]
  assert.equal(
    Array.from(deleted.text)
      .slice(...deleted.mark)
      .join(''),
    'decode',
  )
  assert.equal(
    Array.from(added.text)
      .slice(...added.mark)
      .join(''),
    'verifyToken',
  )
  assert.equal(lines[4].mark, null)
  // 整行几乎都变了时不做词级高亮；码点下标不会切开代理对。
  assert.equal(intralineRange('abc', 'xyz'), null)
  assert.deepEqual(intralineRange('值 😀 a', '值 😀 b'), { start: 4, oldEnd: 5, newEnd: 5 })
  assert.equal(intralineRange('same', 'same'), null)
})

test('the changes tab keeps long lines scrollable and only offers what the runtime can do', async () => {
  const [viewer, changes, inspector] = await Promise.all([
    read('src/features/chat/GitDiffViewer.tsx'),
    read('src/features/chat/InspectorChanges.tsx'),
    read('src/features/chat/SessionInspector.tsx'),
  ])
  // 统一视图：单行不换行（whitespace-pre），列表按最长行撑开，滚动容器横向滚动。
  assert.match(viewer, /function UnifiedDiffRows/)
  assert.match(viewer, /whitespace-pre"/)
  assert.match(viewer, /minWidth: `calc\(\$\{widestChars\}ch/)
  assert.doesNotMatch(
    viewer.slice(viewer.indexOf('function UnifiedRow'), viewer.indexOf('function fileEntry')),
    /truncate|text-ellipsis/,
  )
  // 数据逻辑沿用：本会话走 /file-changes 快照的共用 hook，工作区与旧面板同一个查询键。
  assert.match(changes, /useSessionFileChanges\(\{ sessionId, streaming, requestConfirm \}\)/)
  assert.match(changes, /queryKey: \['task-project-changes', sessionId, cwd, streaming\]/)
  // 没有“在编辑器打开”的能力，只在桌面壳给“在文件管理器中显示”。
  assert.match(changes, /if \(!window\.pisperDesktop \|\| !cwd\) return null/)
  assert.doesNotMatch(changes, /openInEditor|openEditor/)
  assert.match(inspector, /<InspectorChanges/)
  assert.match(
    inspector,
    /<InspectorPlan plan=\{plan\} goal=\{goal\} team=\{team\} activity=\{activity\} \/>/,
  )
})

test('the plan tab shows goal, plan, team and sub-agents only when each has data', async () => {
  const { InspectorPlan } = await import('../../src/features/chat/InspectorPlan.tsx')
  const empty = render(InspectorPlan, { plan: null, goal: null, team: null, activity: [] })
  assert.ok(empty.includes('此会话还没有执行计划。'))
  const html = render(InspectorPlan, {
    plan: { items: [{ id: 'p1', title: '抽出会话刷新模块', status: 'completed' }] },
    goal: {
      id: 'g1',
      objective: '重构登录流程',
      status: 'active',
      mode: 'goal',
      tokensUsed: 12_000,
    },
    team: { tasks: [{ id: 't1', taskName: 'auth-tests', status: 'running' }] },
    activity: [
      { type: 'agent', agent: { id: 'a1', taskName: 'invoice-fixture', status: 'running' } },
      { type: 'agent', agent: { id: 'a1', taskName: 'invoice-fixture', status: 'completed' } },
      { type: 'tool', name: 'bash', status: 'done' },
    ],
  })
  for (const text of [
    '目标',
    '重构登录流程',
    '计划',
    '抽出会话刷新模块',
    '团队',
    'auth-tests',
    '子代理',
  ])
    assert.ok(html.includes(text), text)
  // 同一个子代理多条活动只列一次，取最新状态。
  assert.equal(html.split('invoice-fixture').length - 1, 1)
  assert.match(html, /data-status-glyph="done"/)
})
