import { chromium } from 'playwright-core'

const base = 'http://127.0.0.1:5173'
const browser = await chromium.launch({ channel: 'msedge', headless: true })
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
let sessionId = ''
try {
  await page.goto(base, { waitUntil: 'domcontentloaded' })
  await page.waitForTimeout(4000)
  const created = await page.evaluate(async () => {
    const res = await fetch('/api/sessions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: '截图验证-三点动画' }),
    })
    return res.json()
  })
  sessionId = created.id
  await page.reload({ waitUntil: 'domcontentloaded' })
  await page.waitForTimeout(3500)
  await page.getByRole('button', { name: '截图验证-三点动画', exact: true }).first().click()
  await page.waitForSelector('.focus-composer textarea', { timeout: 8000 })
  await page.waitForTimeout(500)

  await page.fill('.focus-composer textarea', '写一句关于风的话')
  const sentAt = Date.now()
  await page.press('.focus-composer textarea', 'Enter')

  let dotsSeenAt = 0
  let shotTaken = false
  while (Date.now() - sentAt < 30000) {
    const state = await page.evaluate(() => {
      const agents = [...document.querySelectorAll('[data-pisper-role="agent"]')]
      const last = agents.at(-1)
      return {
        hasDots: Boolean(last?.querySelector('.agent-thinking-dots')),
        streaming: last?.getAttribute('data-pisper-streaming') === 'true',
        len: last?.textContent?.length || 0,
      }
    })
    if (state.hasDots && !dotsSeenAt) {
      dotsSeenAt = Date.now() - sentAt
      await page.screenshot({ path: '.tmp-screenshots/typing-dots-gap.png' })
      shotTaken = true
      console.log(`dots appeared at ${dotsSeenAt}ms, screenshot saved`)
    }
    if (!state.streaming && state.len > 20) break
    await page.waitForTimeout(50)
  }
  if (!shotTaken) {
    await page.screenshot({ path: '.tmp-screenshots/typing-dots-gap.png' })
    console.log('dots not observed; fallback screenshot saved')
  }
} finally {
  if (sessionId) {
    await page.evaluate(async (id) => {
      await fetch(`/api/sessions/${id}`, { method: 'DELETE' })
    }, sessionId)
    console.log('test session deleted')
  }
  await browser.close()
}
