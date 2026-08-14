// Capture every product screenshot at the exact 2558x1380 asset size.
// Requires the isolated server (start-isolated-server.mjs) and seeded data
// (seed-demo-data.mjs, which writes generated/screenshot-run/state.json).
// Saves PNGs to generated/screenshot-run/.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { launchScreenshotBrowser } from './browser-launch.mjs'
import { BASE_URL, RUN_DIR } from './screenshot-config.mjs'

const state = JSON.parse(readFileSync(resolve(RUN_DIR, 'state.json'), 'utf8'))

const S1 = state.conversationSessionId
const S2 = state.splitSessionId
const S4 = state.welcomeSessionId
const WF_PUBLISHED = state.workflowId

const browser = await launchScreenshotBrowser()
const context = await browser.newContext({
  viewport: { width: 1279, height: 690 },
  deviceScaleFactor: 2,
  colorScheme: 'light',
  locale: 'zh-CN',
})
const page = await context.newPage()

// Never scan real Codex/Claude config files from the isolated instance.
await page.route('**/api/providers/discovery', (route) =>
  route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: '{"providers":[],"errors":[]}',
  }),
)

// Keep the update page deterministic and independent from GitHub availability.
await page.route('**/api/app-update*', (route) =>
  route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({
      state: 'current',
      branch: 'main',
      message: '当前 Web 源码已同步 main。',
      checkedAt: '2026-08-14T12:00:00.000Z',
    }),
  }),
)

// Stable locale + sidebar baseline for every shot.
await page.addInitScript(() => {
  localStorage.setItem('pisper-language', 'zh-CN')
  localStorage.setItem('pisper-sidebar-collapsed', 'false')
  if (!localStorage.getItem('pisper-ui')) {
    localStorage.setItem(
      'pisper-ui',
      JSON.stringify({
        state: { sidebarCollapsed: false, theme: 'light', density: 'comfortable' },
        version: 0,
      }),
    )
  }
  if (localStorage.getItem('pisper-screenshot-terminal') !== '1') return

  const output = [
    '\u001b[1;34mpisper\u001b[0m \u001b[90mrelease\u001b[0m $ npm run check',
    '',
    '> pisper@0.5.1 check',
    '> npm run typecheck && npm run lint && npm run i18n:check',
    '',
    '\u001b[32mPASS\u001b[0m TypeScript    \u001b[32mPASS\u001b[0m Lint    \u001b[32mPASS\u001b[0m i18n',
    '',
    '\u001b[1;34mpisper\u001b[0m \u001b[90mrelease\u001b[0m $ git status --short',
    '\u001b[32m M\u001b[0m src/features/terminal/TerminalPanel.tsx',
    '\u001b[1;34mpisper\u001b[0m \u001b[90mrelease\u001b[0m $ ',
  ].join('\r\n')
  window.pisperDesktop = {
    platform: 'desktop',
    getAppInfo: async () => ({
      desktop: true,
      packaged: true,
      version: '0.5.1',
      platform: 'desktop',
      arch: 'x86_64',
      releasesUrl: 'https://github.com/ling-kong-ran/pisper/releases',
    }),
    openReleases: async () => true,
    terminalProfiles: async () => [{ id: 'shell', label: 'Shell', default: true }],
    terminalCreate: async (options, onEvent) => {
      window.setTimeout(
        () =>
          onEvent({
            type: 'output',
            terminalId: options.terminalId,
            data: Array.from(new TextEncoder().encode(output)),
          }),
        80,
      )
      return {
        terminalId: options.terminalId,
        profileId: options.profileId,
        cwd: options.cwd,
      }
    },
    terminalWrite: async () => {},
    terminalResize: async () => {},
    terminalClose: async () => true,
    terminalCloseAll: async () => 1,
  }
})

async function goto(path) {
  await page.goto(`${BASE_URL}/#${path}`)
  await page.waitForLoadState('networkidle')
  await page.waitForTimeout(1100)
}

async function chatState({ theme = 'light', activeSession = '', tiled = [] }) {
  await page.goto(`${BASE_URL}/`)
  await page.evaluate(
    ({ theme, activeSession, tiled }) => {
      localStorage.setItem('pisper-theme', theme)
      localStorage.setItem(
        'pisper-ui',
        JSON.stringify({
          state: { sidebarCollapsed: false, theme, density: 'comfortable' },
          version: 0,
        }),
      )
      if (activeSession) localStorage.setItem('pisper-active-session', activeSession)
      else localStorage.removeItem('pisper-active-session')
      localStorage.setItem('pisper-tiled-sessions', JSON.stringify(tiled))
      localStorage.removeItem('pisper-chat-dock-layout-v1')
    },
    { theme, activeSession, tiled },
  )
  await page.reload({ waitUntil: 'domcontentloaded' })
}

async function shot(name) {
  await page.screenshot({ path: resolve(RUN_DIR, `${name}.png`) })
  console.log(`captured ${name}`)
}

// welcome-dark: empty session, dark theme
await chatState({ theme: 'dark', activeSession: S4 })
await goto('/chat')
await shot('welcome-dark')

// chat-grid: two sessions split via the real tab context menu
await chatState({ activeSession: S1, tiled: [S1, S2] })
await goto('/chat')
const secondTab = page.locator('.dv-tab').nth(1)
if (await secondTab.count()) {
  await secondTab.click({ button: 'right' })
  await page.waitForTimeout(600)
  const splitItem = page.locator('.dv-context-menu-item', { hasText: '拆分到右侧' }).first()
  if (await splitItem.count()) {
    await splitItem.click()
    await page.waitForTimeout(1000)
  } else {
    console.log('WARN: split menu item not found')
  }
} else {
  console.log('WARN: second tab not found')
}
await shot('chat-grid')

// chat: focused single session
await chatState({ activeSession: S1 })
await goto('/chat')
await shot('chat')

// turn-label: label editor sits beside the independent-session derive action
const turnLabelButton = page.getByRole('button', { name: '标记此轮' }).last()
await turnLabelButton.click()
await page.locator('.message-label-popover').waitFor({ state: 'visible' })
await page.waitForTimeout(300)
await shot('turn-label')
await page.keyboard.press('Escape')

// session-tree: real Pi branch graph with stable turn labels and active-path projection
const treeEntry = page.locator('.focus-session-tree-entry').first()
await treeEntry.waitFor({ state: 'visible' })
await treeEntry.click()
await page.locator('.session-tree-dialog').waitFor({ state: 'visible' })
await page.waitForTimeout(500)
await shot('session-tree')
await page.keyboard.press('Escape')
await page.locator('.session-tree-dialog').waitFor({ state: 'hidden' })

// session-labels: Ctrl K searches the runtime-owned cross-session label index
await page.keyboard.press('Control+K')
const paletteInput = page.locator('.palette-input input')
await paletteInput.waitFor({ state: 'visible' })
await paletteInput.fill('两周验证计划')
await page.waitForTimeout(500)
await shot('session-labels')
const labelResult = page.locator('.palette-item.has-meta').first()
await labelResult.click()
await page.locator('.virtual-transcript-item.targeted').waitFor({ state: 'visible' })
console.log('verified cross-session label navigation and target highlight')

// Restore the primary conversation before capturing the desktop terminal.
await chatState({ activeSession: S1 })
await goto('/chat')

// terminal: the real desktop TerminalPanel with a deterministic screenshot-only bridge
await page.evaluate(() => {
  localStorage.setItem('pisper-screenshot-terminal', '1')
  localStorage.setItem('pisper-terminal-panel', JSON.stringify({ open: true, height: 260 }))
})
await page.reload({ waitUntil: 'networkidle' })
await page.waitForTimeout(1100)
const terminalEmpty = page.locator('.terminal-empty').first()
await terminalEmpty.waitFor({ state: 'visible' })
await terminalEmpty.click()
try {
  await page.waitForSelector('.terminal-tab', { state: 'visible' })
} catch (error) {
  const debug = await page.evaluate(() => ({
    bridge: Boolean(window.pisperDesktop?.terminalProfiles),
    panel: document.querySelector('.terminal-panel')?.className || '',
    panelText: document.querySelector('.terminal-panel')?.textContent?.trim() || '',
  }))
  throw new Error(`Terminal screenshot did not open: ${JSON.stringify(debug)}`, { cause: error })
}
await page.waitForTimeout(800)
await shot('terminal')
await page.evaluate(() => {
  localStorage.removeItem('pisper-screenshot-terminal')
  localStorage.setItem('pisper-terminal-panel', JSON.stringify({ open: false, height: 260 }))
})
await page.reload({ waitUntil: 'networkidle' })

// history
await goto('/chat/history')
await shot('history')

// assets
await goto('/assets')
await shot('assets')

// channels
await goto('/channels')
await shot('channels')

// schedules
await goto('/schedules')
const scheduleCwd = page.getByLabel('工作目录').first()
if (await scheduleCwd.count()) {
  await scheduleCwd.fill('.')
  await scheduleCwd.blur()
}
await shot('schedules')

// plugins
await goto('/plugins')
await shot('plugins')

// memory: select the demo space
await goto('/memory')
const spaceItem = page.locator('text=项目知识').first()
if (await spaceItem.count()) {
  await spaceItem.click()
  await page.waitForTimeout(800)
}
await shot('memory')

// mcp
await goto('/mcp')
await shot('mcp')

// skills
await goto('/skills')
await shot('skills')

// workflows list
await goto('/workflows')
await shot('workflows')

// workflow builder: open the published workflow
await goto(`/workflows/${WF_PUBLISHED}`)
const workflowCwd = page.getByLabel('工作目录').first()
if (await workflowCwd.count()) {
  await workflowCwd.fill('.')
  await workflowCwd.blur()
}
await page.waitForTimeout(600)
await shot('workflow-builder')

// config sections
async function configSection(name, label) {
  await goto('/config')
  const tab = page.getByRole('button', { name: label }).first()
  if (await tab.count()) {
    await tab.click()
    await page.waitForTimeout(700)
  }
  await shot(name)
}
await configSection('config', '模型配置')
await configSection('config-notifications', '通知设置')
await configSection('config-interface', '界面设置')
await configSection('config-desktop-pet', '桌面宠物')
await configSection('config-updates', '应用更新')

await browser.close()
console.log('all captured')
