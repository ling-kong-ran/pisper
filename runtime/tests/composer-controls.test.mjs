import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('labeled and icon controls hide the Radix overlay even while disabled', async () => {
  const component = await readFile('src/features/chat/FocusRuntimeControls.tsx', 'utf8')

  assert.match(component, /const ICON_SELECT_CLASSES =/)
  assert.match(component, /showLabel \? LABEL_SELECT_CLASSES : ICON_SELECT_CLASSES/)
  assert.ok(
    component.includes(
      "modelLabelOnly ? currentModel?.label || value.split('/').at(-1) : currentLabel",
    ),
  )
  assert.match(component, /t\('chat:focusSession.toolbarModel'\)/)
  assert.match(component, /levelLabel\(current\)/)
  assert.equal(
    (
      component.match(
        /<AppSelect\s+className="absolute inset-0 !size-full cursor-pointer !opacity-0"/g,
      ) || []
    ).length,
    2,
  )
  assert.match(component, /focus-within:ring-2/)
})

// 规格 v2 第 4.3 节：工具行固定为 [+] [盾牌芯片] … [模型芯片] [发送]。原来可配置的工具条
// （内联 / 收纳分配、按宽度自动收纳、自定义对话框）与“+”托盘、详情行一起退役；等价的保护改为：
// 四个控件的顺序与“+”菜单的内容，以及退役件不再出现。
test('composer toolbar is fixed: + menu, shield chip, model chip and send', async () => {
  const session = await readFile('src/features/chat/FocusSession.tsx', 'utf8')
  assert.match(session, /focus-composer[^"\n]*\[&_textarea\]:\[outline:0\]!/)
  const toolbar = session.slice(session.indexOf('data-composer-toolbar'))
  const order = ['{plusMenu}', '{modeChip}', '{modelChip}', '<ComposerSendButton'].map((token) =>
    toolbar.indexOf(token),
  )
  assert.ok(
    order.every((index, position) => index > 0 && (position === 0 || index > order[position - 1])),
    `toolbar order ${order}`,
  )
  // “+”菜单：附件、技能与工具、生成图片（有能力时）、语音输入（F8）；手机运行中另有“插话发送”。
  for (const item of ['attachment', 'resource', 'visual', 'voice'])
    assert.match(session, new RegExp(`data-composer-plus-item="${item}"`))
  assert.match(session, /phonePage && streaming && \(\s*<DropdownMenuItem\s+data-composer-steer/)
  assert.match(session, /onSelect=\{\(\) => voiceControlRef\.current\?\.\(\)\}/)
  assert.match(
    session,
    /<VoiceInputControl[\s\S]*?variant="menu"[\s\S]*?controlRef=\{voiceControlRef\}/,
  )
  // 上下文用量在模型芯片弹层里（ContextUsageSection），不再有详情行与上下文环。
  assert.match(session, /usage=\{\s*<ContextUsageSection/)
  for (const retired of [
    'toolbarAllocation',
    'ComposerToolTray',
    'ComposerToolbarSettings',
    'composer-details',
    'ContextUsageIndicator',
    'SessionUsageMetrics',
    'ComposerStatusPill',
    'data-composer-hint',
    'command-palette-trigger',
  ])
    assert.doesNotMatch(session, new RegExp(retired), retired)
  for (const file of [
    'composer-toolbar-layout.ts',
    'composer-toolbar-store.ts',
    'use-composer-toolbar-capacity.ts',
    'ComposerToolbarSettings.tsx',
    'ComposerToolTray.tsx',
    'GoalModeControl.tsx',
  ])
    await assert.rejects(readFile(`src/features/chat/${file}`, 'utf8'), /ENOENT/, file)
  assert.ok(session.indexOf('<ComposerCommandMenu') < session.indexOf('<textarea'))
})

test('composer plain Enter submits, Shift+Enter inserts a newline, and IME composition never submits', async () => {
  const session = await readFile('src/features/chat/FocusSession.tsx', 'utf8')

  // 默认仍为 Enter；自定义绑定统一经精确匹配，不能绕过组词与换行保护。
  const { DEFAULT_SHORTCUTS, matchesShortcut } = await import('../../shared/shortcuts.mjs')
  assert.equal(DEFAULT_SHORTCUTS.sendMessage, 'Enter')
  assert.equal(matchesShortcut({ code: 'Enter' }, DEFAULT_SHORTCUTS.sendMessage), true)
  assert.equal(
    matchesShortcut({ code: 'Enter', shiftKey: true }, DEFAULT_SHORTCUTS.sendMessage),
    false,
  )
  assert.equal(
    matchesShortcut({ code: 'Enter', isComposing: true }, DEFAULT_SHORTCUTS.sendMessage),
    false,
  )
  assert.equal(matchesShortcut({ code: 'Enter' }, 'Mod+Enter'), false)
  assert.equal(matchesShortcut({ code: 'Enter', ctrlKey: true }, 'Mod+Enter'), true)
  assert.equal(
    matchesShortcut({ code: 'Enter', ctrlKey: true, shiftKey: true }, 'Mod+Enter'),
    false,
  )
  assert.equal(matchesShortcut({ code: 'Enter', ctrlKey: true, keyCode: 229 }, 'Mod+Enter'), false)
  assert.match(
    session,
    /!composing && matchesShortcut\(event\.nativeEvent, shortcuts\.sendMessage\)/,
  )
  assert.match(session, /useShortcutStore\(\(state\) => state\.bindings\)/)
  assert.match(session, /event\.currentTarget\.form\?\.requestSubmit\(\)/)
  assert.match(
    session,
    /enterKeyHint=\{\s*mobileLayout && shortcuts\.sendMessage === 'Enter' \? 'send' : 'enter'\s*\}/,
  )
  // IME 组词保护双保险：Chromium 靠 isComposing；Mac WebKit 的确认 Enter
  // 在 compositionend 之后派发，靠自行跟踪的 imeComposingRef 延迟复位覆盖。
  assert.match(session, /event\.nativeEvent\.isComposing \|\| imeComposingRef\.current/)
  assert.match(session, /onCompositionStart/)
  assert.match(session, /onCompositionEnd/)
})

test('composer exposes a session thinking-level control wired to the shared API', async () => {
  const [controls, session, api] = await Promise.all([
    readFile('src/features/chat/FocusRuntimeControls.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/chat-api.ts', 'utf8'),
  ])

  assert.match(controls, /export function SessionThinkingSelect/)
  assert.match(controls, /session-thinking-select/)
  assert.match(session, /ModelThinkingControl/)
  assert.match(session, /onThinkingLevelChange/)
  assert.match(api, /getThinkingLevel/)
  assert.match(api, /setThinkingLevel/)
  assert.match(api, /thinking-level/)
  assert.match(session, /onThinkingChange=\{onThinkingLevelChange\}/)
  assert.match(session, /onModelChange=\{onModelChange\}/)
  assert.match(controls, /<SessionModelSelect[\s\S]*onChange=\{onModelChange\}/)
  assert.match(controls, /type="range"/)
  assert.match(controls, /await onThinkingChange\(level\)/)
  assert.match(controls, /onPointerUp=/)
  assert.match(controls, /onKeyUp=/)
  assert.match(controls, /levels\.length < 2/)
  assert.match(controls, /disabled=\{disabled \|\| loading \|\| !supported \|\| fixed\}/)
  assert.match(controls, /role="status"/)
  assert.doesNotMatch(session, /thinking:\s*\(/)
})

test('the shield chip combines approval mode and run mode in one portaled popover', async () => {
  const chip = await readFile('src/features/chat/ComposerModeChip.tsx', 'utf8')
  // 桌面是 Radix Popover（走 portal，不被输入区裁切），手机会话页同一份内容换成底部表单（规格 7.2，
  // PopoverSheet）；两组选项都是 radiogroup / radio。
  assert.match(
    chip,
    /<PopoverSheet\s+sheet=\{sheet\}[\s\S]{0,120}open=\{open\}\s+onOpenChange=\{setOpen\}[\s\S]{0,80}className="composer-mode-popover/,
  )
  const popoverSheet = await readFile('src/components/app/PopoverSheet.tsx', 'utf8')
  assert.match(
    popoverSheet,
    /if \(!sheet\)\s*return \(\s*<Popover open=\{open\} onOpenChange=\{setOpen\}>/,
  )
  assert.match(
    popoverSheet,
    /<Sheet open=\{open\} onOpenChange=\{setOpen\}>\s*<SheetContent\s+side="bottom"/,
  )
  const focus = await readFile('src/features/chat/FocusSession.tsx', 'utf8')
  assert.match(focus, /<ComposerModeChip\s+sheet=\{phonePage\}/)
  assert.match(focus, /<ModelThinkingControl\s+sheet=\{phonePage\}/)
  assert.equal((chip.match(/<Section\b/g) || []).length, 2)
  assert.match(chip, /role="radiogroup"/)
  assert.match(chip, /role="radio"/)
  assert.match(chip, /aria-checked=\{selected\}/)
  // 选审批模式即关闭弹层；Goal / Team 留下来填预算。
  assert.match(chip, /setOpen\(false\)\s*onApprovalModeChange\(mode\)/)
  assert.match(chip, /if \(mode === 'plan'\) setOpen\(false\)/)
  // 芯片只用中性色：完全访问不染琥珀或警示色（规格 6.1）。
  assert.doesNotMatch(chip, /warning|st-attn|danger/)
})

test('Plan, Goal and Team stay real execution choices, distinct from approval mode', async () => {
  const [chip, session] = await Promise.all([
    readFile('src/features/chat/ComposerModeChip.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
  ])
  const { nextComposerRunMode } = await import('../../src/features/chat/composer-run-mode.ts')
  assert.match(chip, /value: 'plan'/)
  assert.match(chip, /value: 'goal' as const/)
  assert.match(chip, /goalsAvailable && teamAvailable/)
  // 非 Plan 时芯片前面写出模式名（“Goal · 审批后写入”）。
  assert.match(
    chip,
    /const chipLabel = runShort \? `\$\{runShort\} · \$\{approval\.label\}` : approval\.label/,
  )
  assert.match(session, /onRunModeChange\(nextMode\)/)
  assert.match(session, /approvalMode=\{executionMode\}/)
  assert.match(session, /onApprovalModeChange=\{onExecutionModeChange\}/)
  // ⇧Tab 在输入框里循环：Plan → Goal → Team → Plan，不可用的方式跳过。
  const all = { goalsAvailable: true, teamAvailable: true }
  assert.equal(nextComposerRunMode('plan', all), 'goal')
  assert.equal(nextComposerRunMode('goal', all), 'team')
  assert.equal(nextComposerRunMode('team', all), 'plan')
  assert.equal(nextComposerRunMode('goal', { goalsAvailable: true, teamAvailable: false }), 'plan')
  assert.equal(nextComposerRunMode('plan', { goalsAvailable: false, teamAvailable: true }), null)
  assert.match(session, /event\.key === 'Tab' &&\s*event\.shiftKey/)
  assert.match(session, /!composing &&\s*!streaming\s*\)/)
})
