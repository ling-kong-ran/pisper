import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

async function source(path) {
  return readFile(new URL(path, import.meta.url), 'utf8')
}

test('session usage stays scoped to each dock panel and updates over SSE', async () => {
  const [dock, focus, dispatch, sync, stateTypes, defaults] = await Promise.all([
    source('../../src/features/chat/ChatDock.tsx'),
    source('../../src/features/chat/FocusSession.tsx'),
    source('../../src/features/chat/stream-event-dispatch.ts'),
    source('../../src/features/chat/use-live-session-sync.ts'),
    source('../../src/types/chat.ts'),
    source('../../src/lib/session-state.ts'),
  ])

  assert.match(stateTypes, /sessionUsage: EntityRecord \| null/)
  assert.match(defaults, /sessionUsage: null/)
  assert.match(dock, /sessionUsage=\{state\.sessionUsage\}/)
  assert.match(dock, /loadThinkingLevel/)
  // 用量在模型芯片弹层的用量段里（规格 v2 第 2.5 节），按会话取自 sessionUsage。
  assert.match(focus, /<ContextUsageSection[\s\S]*?sessionUsage=\{sessionUsage\}[\s\S]*?\/>/)
  assert.match(focus, /runtimeFeatureAvailable\(capabilities, 'plans'\)/)
  assert.match(dispatch, /event === 'session_usage'/)
  assert.match(dispatch, /sessionUsage: data/)
  assert.match(sync, /sessionUsage: data\.sessionUsage \?\? current\.sessionUsage \?\? null/)
  assert.match(sync, /sessionUsage: data\.sessionUsage \?\? latest\.sessionUsage \?\? null/)
})

// 规格 v2 第 2.5 / 4.3 节：输入框下方的详情行（用量读数、计划读数、语音模式键）退役。
// 上下文用量、本会话 tok 与缓存命中进模型芯片弹层；计划进度在顶栏下方；语音模式在标题菜单；
// 语音输入在“+”菜单（录音中的状态键仍挂在工具行，F8 照常）。
test('model chip popover carries usage readings and voice controls stay mounted in the toolbar', async () => {
  const [focus, controls, css] = await Promise.all([
    source('../../src/features/chat/FocusSession.tsx'),
    source('../../src/features/chat/FocusRuntimeControls.tsx'),
    source('../../src/index.css'),
  ])
  assert.match(focus, /usage=\{\s*<ContextUsageSection[\s\S]*?usage=\{contextUsage\}/)
  assert.match(controls, /data-usage-row=\{key\}/)
  assert.match(controls, /t\('board:composer\.contextUsage'\)/)
  assert.match(controls, /t\('board:composer\.sessionTokens'\)/)
  assert.match(controls, /t\('board:composer\.cacheHitLabel'\)/)
  // 缓存命中未上报时写“—”并说明原因，不拿 0 冒充。
  assert.match(
    controls,
    /cacheRateKnown \? `\$\{Math\.round\(Math\.max\(0, cacheHitRate\)\)\}%` : '—'/,
  )
  assert.match(controls, /chat:focusSession\.cacheHitRateUnavailable/)
  assert.match(controls, /formatTokenCount\(sessionUsage\?\.reasoning\)/)
  assert.match(controls, /\{usage\}\s*<\/PopoverSheet>/)
  // 详情行与语音模式键退役；语音输入组件常驻工具行（menu 形态），录音状态不会因菜单关闭而卸载。
  assert.doesNotMatch(focus, /composer-details|detailsOpen|usageBlock|AudioLines/)
  assert.match(focus, /\{voiceInput\}\s*<div className="flex min-w-0 shrink items-center/)
  assert.match(focus, /onVoiceMode: \(\) => setVoiceModeOpen\(true\)/)
  assert.doesNotMatch(css, /\.session-usage-metrics|\.focus-composer-meta|\.header-workspace/)
})
