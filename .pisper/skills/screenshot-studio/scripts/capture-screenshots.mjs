// Capture every product screenshot at the exact 2558x1380 asset size.
// Requires the isolated server (start-isolated-server.mjs) and seeded data
// (seed-demo-data.mjs, which writes generated/screenshot-run/state.json).
// Saves PNGs to generated/screenshot-run/.
import { chromium } from 'playwright-core'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const ROOT = resolve(import.meta.dirname, '../../../..')
const baseUrl = `http://127.0.0.1:${process.env.SCREENSHOT_PORT || 5180}`
const outDir = resolve(ROOT, 'generated/screenshot-run')
const state = JSON.parse(readFileSync(resolve(outDir, 'state.json'), 'utf8'))

const S1 = state.conversationSessionId
const S2 = state.splitSessionId
const S4 = state.welcomeSessionId
const WF_PUBLISHED = state.workflowId

const browser = await chromium.launch({
  headless: true,
  executablePath: 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
})
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

// Stable locale + sidebar baseline for every shot.
await page.addInitScript(() => {
  localStorage.setItem('pisper-language', 'zh-CN')
  localStorage.setItem('pisper-sidebar-collapsed', 'false')
})

async function goto(path) {
  await page.goto(`${baseUrl}/#${path}`)
  await page.waitForLoadState('networkidle')
  await page.waitForTimeout(1100)
}

async function chatState({ theme = 'light', activeSession = '', tiled = [] }) {
  await page.goto(`${baseUrl}/`)
  await page.evaluate(
    ({ theme, activeSession, tiled }) => {
      localStorage.setItem('pisper-theme', theme)
      if (activeSession) localStorage.setItem('pisper-active-session', activeSession)
      else localStorage.removeItem('pisper-active-session')
      localStorage.setItem('pisper-tiled-sessions', JSON.stringify(tiled))
      localStorage.removeItem('pisper-chat-dock-layout-v1')
    },
    { theme, activeSession, tiled },
  )
}

async function shot(name) {
  await page.screenshot({ path: resolve(outDir, `${name}.png`) })
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
