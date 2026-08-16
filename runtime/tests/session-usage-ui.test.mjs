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
  assert.match(focus, /<SessionUsageMetrics usage=\{sessionUsage\} plan=\{plan\} \/>/)
  assert.match(dispatch, /event === 'session_usage'/)
  assert.match(dispatch, /sessionUsage: data/)
  assert.match(sync, /sessionUsage: data\.sessionUsage \?\? current\.sessionUsage \?\? null/)
  assert.match(sync, /sessionUsage: data\.sessionUsage \?\? latest\.sessionUsage \?\? null/)
})

test('composer renders unframed metrics as a separate row below the input controls', async () => {
  const [focus, controls, css] = await Promise.all([
    source('../../src/features/chat/FocusSession.tsx'),
    source('../../src/features/chat/FocusRuntimeControls.tsx'),
    source('../../src/index.css'),
  ])

  assert.match(
    focus,
    /composer-workspace-status[\s\S]*<SessionUsageMetrics usage=\{sessionUsage\} plan=\{plan\} \/>[\s\S]*<\/form>/,
  )
  assert.match(controls, /cacheHitRate/)
  assert.match(controls, /usage\?\.processedTokens/)
  assert.match(controls, /formatTokenCount\(processedTokens\)/)
  assert.match(controls, /formatTokenCount\(usage\?\.reasoning\)/)
  assert.match(controls, /completedPlanItems/)
  assert.match(controls, /chat:planBoard\.progress/)
  assert.match(controls, /className="session-plan-progress[^"\n]*"/)
  assert.match(controls, /chat:planBoard\.openCurrentPlan/)
  assert.match(controls, /<PopoverTrigger asChild>/)
  assert.match(controls, /<PlanBoard plan=\{plan \?\? null\} \/>/)
  assert.match(controls, /session-usage-metrics[^"\n]*justify-start/)
  const metricsRow = focus.match(/<div className="flex min-w-0 min-h-\[26px\][^"\n]*"/)?.[0] || ''
  assert.ok(metricsRow)
  assert.doesNotMatch(metricsRow, /border|bg-|shadow/)
  assert.match(focus, /composer-workspace-status[^"\n]*border-0[^"\n]*bg-transparent/)
  assert.match(controls, /session-plan-popover[^"\n]*max-h-/)
  assert.doesNotMatch(
    css,
    /\.session-usage-metrics|\.focus-composer-meta|\.composer-workspace-status/,
  )
})
