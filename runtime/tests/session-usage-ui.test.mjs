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
  // ZCode 式精简：composer 底部的用量指标行已移除，用量与缓存命中率
  // 经上下文用量弹窗（详细统计面板）展示；数据链路保持不变。
  assert.doesNotMatch(focus, /<SessionUsageMetrics/)
  assert.doesNotMatch(focus, /composer-workspace-status/)
  assert.match(dispatch, /event === 'session_usage'/)
  assert.match(dispatch, /sessionUsage: data/)
  assert.match(sync, /sessionUsage: data\.sessionUsage \?\? current\.sessionUsage \?\? null/)
  assert.match(sync, /sessionUsage: data\.sessionUsage \?\? latest\.sessionUsage \?\? null/)
})

test('usage metrics surface through the context usage popover instead of a composer row', async () => {
  const [focus, controls, css] = await Promise.all([
    source('../../src/features/chat/FocusSession.tsx'),
    source('../../src/features/chat/FocusRuntimeControls.tsx'),
    source('../../src/index.css'),
  ])

  // composer 不再渲染独立指标行；统计入口在用量指示弹窗。
  assert.doesNotMatch(focus, /<SessionUsageMetrics/)
  assert.match(controls, /cacheHitRate/)
  assert.match(controls, /usage\?\.processedTokens/)
  assert.match(controls, /formatTokenCount\(processedTokens\)/)
  assert.match(controls, /formatTokenCount\(usage\?\.reasoning\)/)
  assert.match(controls, /export function SessionStatsPanel/)
  assert.match(controls, /chat:focusSession\.statAvgFirstToken/)
  assert.match(controls, /cacheKnown \? formatTokenCount/)
  assert.match(controls, /<PopoverTrigger asChild>/)
  assert.match(controls, /session-plan-popover[^"\n]*max-h-/)
  assert.doesNotMatch(
    css,
    /\.session-usage-metrics|\.focus-composer-meta|\.composer-workspace-status/,
  )
})
