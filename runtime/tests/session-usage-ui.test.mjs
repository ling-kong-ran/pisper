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

test('composer renders the metrics as a separate row below the input controls', async () => {
  const [focus, controls, css] = await Promise.all([
    source('../../src/features/chat/FocusSession.tsx'),
    source('../../src/features/chat/FocusRuntimeControls.tsx'),
    source('../../src/index.css'),
  ])

  assert.match(
    focus,
    /<\/div>\s*<SessionUsageMetrics usage=\{sessionUsage\} plan=\{plan\} \/>\s*<\/form>/,
  )
  assert.match(controls, /cacheHitRate/)
  assert.match(controls, /usage\?\.processedTokens/)
  assert.match(controls, /formatTokenCount\(processedTokens\)/)
  assert.match(controls, /formatTokenCount\(usage\?\.reasoning\)/)
  assert.match(controls, /completedPlanItems/)
  assert.match(controls, /chat:planBoard\.progress/)
  assert.match(controls, /className="session-plan-progress"/)
  assert.match(css, /\.session-usage-metrics \{[^}]*justify-content: flex-start/)
})
