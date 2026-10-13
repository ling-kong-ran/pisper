// 独立渲染真实追忆组件；HTTP 响应全部来自构造数据，不启动或连接用户 Runtime。
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { createServer } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { chromium } from 'playwright-core'

const root = resolve(import.meta.dirname, '..')
const cacheDir = await mkdtemp(join(tmpdir(), 'pisper-tree-ui-'))
const fixtureId = resolve(root, 'scripts/session-tree-fixture.tsx').replaceAll('\\', '/')
const fixture = `
import { useCallback, useRef, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { SessionTreeDialog } from '@/features/chat/components/session/SessionTreeDialog'
import { useLiveSessionSync } from '@/features/chat/hooks/use-live-session-sync'
import { applySessionUpdate, DEFAULT_SESSION_STATE } from '@/lib/session/session-state'
import '@/app/i18n/i18n'
import '@/index.css'
function Fixture() {
  const [open, setOpen] = useState(true)
  const [refreshed, setRefreshed] = useState(false)
  const [states, setStates] = useState({ 'fixture-session': {
    ...DEFAULT_SESSION_STATE,
    messages: [{ id: 'left-final', role: 'agent', text: 'Deleted branch stale message' }],
  } })
  const statesRef = useRef(states)
  const streamsRef = useRef(new Set())
  const generationsRef = useRef(new Map())
  const updateSessionState = useCallback((id, update) => {
    const next = { ...statesRef.current, [id]: applySessionUpdate(statesRef.current[id], update) }
    statesRef.current = next
    setStates(next)
  }, [])
  const updateSessions = useCallback(() => [], [])
  const { loadSessionMessages } = useLiveSessionSync({
    sessionStates: states, sessionStatesRef: statesRef,
    localStreamSessionsRef: streamsRef, streamGenerationRef: generationsRef,
    updateSessionState, updateSessions,
  })
  return <>
    <output data-testid="message-refresh">{refreshed ? 'refreshed' : 'original'}</output>
    <output data-testid="message-ids">{states['fixture-session'].messages.map((message) => message.id).join(',')}</output>
    <SessionTreeDialog open={open} sessionId="fixture-session" streaming={false}
      onClose={() => setOpen(false)} onCreateChildSession={() => {}}
      onNavigated={async (text) => {
        if (text !== null) throw new Error('Deletion must refresh without editor text')
        await loadSessionMessages('fixture-session', { force: true, throwOnError: true })
        setRefreshed(true)
      }} />
  </>
}
createRoot(document.getElementById('root')).render(<Fixture />)
`
const server = await createServer({
  root,
  configFile: false,
  cacheDir,
  logLevel: 'error',
  resolve: { alias: { '@': join(root, 'src'), '@shared': join(root, 'shared') } },
  plugins: [
    react(),
    tailwindcss(),
    {
      name: 'isolated-session-tree-fixture',
      resolveId(id) {
        if (id === '/session-tree-fixture.tsx' || id === fixtureId) return fixtureId
      },
      load(id) {
        if (id === fixtureId) return fixture
      },
      configureServer(vite) {
        vite.middlewares.use('/fixture.html', async (_request, response, next) => {
          try {
            const html = await vite.transformIndexHtml(
              '/fixture.html',
              '<!doctype html><html><body><div id="root"></div><script type="module" src="/session-tree-fixture.tsx"></script></body></html>',
            )
            response.setHeader('Content-Type', 'text/html')
            response.end(html)
          } catch (error) {
            next(error)
          }
        })
      },
    },
  ],
  server: { host: '127.0.0.1', port: 0, watch: null, hmr: false },
})

function node(id, parentId, kind, active = false, extra = {}) {
  return {
    id,
    parentId,
    type: 'message',
    kind,
    role: kind,
    text: `Fixture ${id}`,
    status: 'completed',
    label: '',
    timestamp: '2026-01-01T12:00:00Z',
    active,
    leaf: id === 'right-answer',
    branchPoint: id === 'shared-answer',
    ...extra,
  }
}

function nodes() {
  return [
    node('settings', null, 'settings', true, { type: 'model_change' }),
    node('shared-user', 'settings', 'user', true),
    node('shared-answer', 'shared-user', 'assistant', true),
    node('left-user', 'shared-answer', 'user'),
    node('left-answer', 'left-user', 'assistant', false, { label: 'Left branch marker' }),
    node('left-tool', 'left-answer', 'tool', false, { role: 'toolResult' }),
    node('left-followup', 'left-tool', 'user'),
    node('left-final', 'left-followup', 'assistant'),
    node('right-user', 'shared-answer', 'user', true),
    node('right-answer', 'right-user', 'assistant', true),
  ]
}

let browser
try {
  await server.listen()
  const base = server.resolvedUrls.local[0]
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_SMOKE_BROWSER
      ? { executablePath: process.env.PISPER_SMOKE_BROWSER }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })

  async function scenario({ streaming = false, rejectDelete = false, failRefresh = '' } = {}) {
    const context = await browser.newContext({ viewport: { width: 1280, height: 900 } })
    const page = await context.newPage()
    page.setDefaultTimeout(12_000)
    let entries = nodes()
    const deletes = []
    const errors = []
    const reads = { tree: 0, messages: 0 }
    page.on('pageerror', (error) => errors.push(error.message))
    await page.route(`${base}api/**`, (route) => {
      const path = new URL(route.request().url()).pathname
      if (path.endsWith('/tree/branches/left-user')) {
        assert.equal(route.request().method(), 'DELETE')
        deletes.push('left-user')
        if (rejectDelete) {
          return route.fulfill({ status: 409, json: { error: 'Fixture session is busy' } })
        }
        const deletedEntryIds = [
          'left-user',
          'left-answer',
          'left-tool',
          'left-followup',
          'left-final',
        ]
        entries = entries.filter((entry) => !deletedEntryIds.includes(entry.id))
        return route.fulfill({ json: { deletedEntryIds, leafId: 'right-answer' } })
      }
      if (path.endsWith('/tree')) {
        reads.tree += 1
        if (failRefresh === 'tree' && reads.tree === 2) {
          return route.fulfill({ status: 500, json: { error: 'Fixture tree refresh failed' } })
        }
        return route.fulfill({
          json: {
            sessionId: 'fixture-session',
            nodes: entries,
            leafId: 'right-answer',
            nodeCount: entries.length,
            branchCount: 2,
            streaming,
            lineage: null,
          },
        })
      }
      if (path.endsWith('/messages')) {
        reads.messages += 1
        if (failRefresh === 'messages' && reads.messages === 1) {
          return route.fulfill({ status: 500, json: { error: 'Fixture message refresh failed' } })
        }
        return route.fulfill({
          json: {
            messages: [{ id: 'right-answer', role: 'agent', text: 'Surviving sibling' }],
            pageInfo: { start: 0, hasMore: false, nextCursor: null },
          },
        })
      }
      if (path === '/api/sessions') return route.fulfill({ json: { sessions: [] } })
      throw new Error(`Unexpected fixture request: ${path}`)
    })
    await page.goto(`${base}fixture.html`)
    const dialog = page.getByRole('dialog', { name: '追忆', exact: true })
    await dialog.locator('[data-pisper-tree-entry="left-user"]').click()
    return { context, page, dialog, deletes, errors, reads }
  }

  const success = await scenario()
  try {
    const { page, dialog, deletes, errors } = success
    await dialog
      .getByRole('textbox', { name: '搜索标记和节点内容', exact: true })
      .fill('Left branch marker')
    await dialog.locator('[data-pisper-tree-entry="left-answer"]').click()
    assert.equal(await dialog.locator('[data-pisper-tree-entry="right-answer"]').count(), 0)
    await dialog.getByRole('button', { name: '清除搜索', exact: true }).click()
    await dialog.locator('[data-pisper-tree-entry="right-answer"]').waitFor()
    const leftBranch = await dialog.locator('[data-pisper-tree-entry="left-user"]').boundingBox()
    const rightBranch = await dialog.locator('[data-pisper-tree-entry="right-user"]').boundingBox()
    assert.ok(
      leftBranch && rightBranch && rightBranch.x - leftBranch.x - leftBranch.width >= 12,
      'Sibling branches need visible separation',
    )
    if (process.env.PISPER_TREE_SCREENSHOT) {
      await page.screenshot({ path: process.env.PISPER_TREE_SCREENSHOT, animations: 'disabled' })
    }
    await dialog.locator('[data-pisper-tree-entry="left-user"]').click()
    const remove = dialog.getByRole('button', { name: '删除所选分支', exact: true })
    await remove.click()
    const confirmation = page.getByRole('alertdialog')
    await confirmation.getByRole('button', { name: '取消', exact: true }).click()
    await confirmation.waitFor({ state: 'hidden' })
    assert.deepEqual(deletes, [], 'Cancelling must not mutate the tree')
    assert.equal(
      await remove.evaluate((button) => document.activeElement === button),
      true,
      'Cancelling restores keyboard focus to the delete action',
    )
    await dialog.locator('[data-pisper-tree-entry="left-final"]').waitFor()

    const refreshStarted = Promise.withResolvers()
    const refreshGate = Promise.withResolvers()
    await page.route(`${base}api/sessions/fixture-session/messages**`, async (route) => {
      refreshStarted.resolve()
      await refreshGate.promise
      await route.fulfill({
        json: {
          messages: [{ id: 'right-answer', role: 'agent', text: 'Surviving sibling' }],
          pageInfo: { start: 0, hasMore: false, nextCursor: null },
        },
      })
    })
    await remove.click()
    await confirmation.getByRole('button', { name: '确认删除分支', exact: true }).click()
    let refreshTimeout
    try {
      await Promise.race([
        refreshStarted.promise,
        new Promise((_, reject) => {
          refreshTimeout = setTimeout(
            () => reject(new Error('Message refresh never started')),
            12_000,
          )
        }),
      ])
      assert.deepEqual(deletes, ['left-user'])
      assert.equal(
        await confirmation.getByRole('button', { name: '正在删除…', exact: true }).isDisabled(),
        true,
        'Deletion must stay pending until the message refresh completes',
      )
    } finally {
      clearTimeout(refreshTimeout)
      refreshGate.resolve()
    }
    await confirmation.waitFor({ state: 'hidden' })
    await page.getByTestId('message-refresh').getByText('refreshed', { exact: true }).waitFor()
    for (const id of ['left-user', 'left-answer', 'left-followup', 'left-final']) {
      assert.equal(await dialog.locator(`[data-pisper-tree-entry="${id}"]`).count(), 0)
    }
    await dialog.locator('[data-pisper-tree-entry="shared-answer"]').waitFor()
    await dialog.locator('[data-pisper-tree-entry="right-answer"]').waitFor()
    assert.deepEqual(errors, [])
  } finally {
    await success.context.close()
  }

  const failed = await scenario({ rejectDelete: true })
  try {
    await failed.dialog.getByRole('button', { name: '删除所选分支', exact: true }).click()
    const confirmation = failed.page.getByRole('alertdialog')
    await confirmation.getByRole('button', { name: '确认删除分支', exact: true }).click()
    await confirmation.getByRole('alert').getByText('Fixture session is busy').waitFor()
    await confirmation.getByRole('button', { name: '取消', exact: true }).click()
    await failed.dialog.locator('[data-pisper-tree-entry="left-final"]').waitFor()
    assert.equal(await failed.page.getByTestId('message-refresh').textContent(), 'original')
    assert.deepEqual(failed.errors, [])
  } finally {
    await failed.context.close()
  }

  for (const failRefresh of ['messages', 'tree']) {
    const recovery = await scenario({ failRefresh })
    try {
      await recovery.dialog.getByRole('button', { name: '删除所选分支', exact: true }).click()
      const confirmation = recovery.page.getByRole('alertdialog')
      await confirmation.getByRole('button', { name: '确认删除分支', exact: true }).click()
      await confirmation.waitFor({ state: 'hidden' })
      await recovery.dialog
        .getByRole('alert')
        .getByText(/分支已删除，但刷新未完成/)
        .waitFor()
      if (failRefresh === 'messages') {
        assert.equal(await recovery.page.getByTestId('message-refresh').textContent(), 'original')
        assert.equal(await recovery.page.getByTestId('message-ids').textContent(), 'left-final')
      }
      await recovery.dialog.getByRole('button', { name: '重试', exact: true }).click()
      await recovery.page
        .getByTestId('message-ids')
        .getByText('right-answer', { exact: true })
        .waitFor()
      await recovery.dialog.locator('[data-pisper-tree-entry="right-answer"]').waitFor()
      await recovery.dialog.getByRole('alert').waitFor({ state: 'hidden' })
      assert.deepEqual(recovery.reads, { tree: 3, messages: 2 }, 'Retry refreshes both snapshots')
      assert.deepEqual(recovery.deletes, ['left-user'], 'Retry must not delete a second time')
      assert.deepEqual(recovery.errors, [])
    } finally {
      await recovery.context.close()
    }
  }

  const busy = await scenario({ streaming: true })
  try {
    assert.equal(
      await busy.dialog.getByRole('button', { name: '删除所选分支', exact: true }).isDisabled(),
      true,
    )
    await busy.dialog.getByRole('tab', { name: '全部', exact: true }).click()
    await busy.dialog.locator('[data-pisper-tree-entry="settings"]').click()
    assert.equal(
      await busy.dialog.getByRole('button', { name: '删除所选分支', exact: true }).count(),
      0,
    )
    await busy.dialog.locator('[data-pisper-tree-entry="left-tool"]').click()
    assert.equal(
      await busy.dialog.getByRole('button', { name: '删除所选分支', exact: true }).count(),
      0,
    )
    assert.deepEqual(busy.deletes, [])
    assert.deepEqual(busy.errors, [])
  } finally {
    await busy.context.close()
  }
  console.log(
    'Session tree UI: cancel, descendants, siblings, message refresh/retry, failure and busy guards passed',
  )
} finally {
  await browser?.close()
  await server.close()
  await rm(cacheDir, { recursive: true, force: true })
}
