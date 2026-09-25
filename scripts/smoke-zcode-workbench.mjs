import assert from 'node:assert/strict'
import { mkdtemp, mkdir } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { chromium } from 'playwright-core'
import { writeFile } from 'node:fs/promises'
if (process.env.PISPER_SMOKE_ISOLATED !== '1')
  throw new Error('Use an isolated runtime and set PISPER_SMOKE_ISOLATED=1')
const base = new URL(process.env.PISPER_SMOKE_BASE_URL || 'http://127.0.0.1:5189')
assert(['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname))
const output =
  process.env.PISPER_SMOKE_OUTPUT || (await mkdtemp(path.join(tmpdir(), 'pisper-zcode-smoke-')))
await mkdir(output, { recursive: true })
const browser = await chromium.launch({
  headless: true,
  ...(process.env.PISPER_SMOKE_BROWSER
    ? { executablePath: process.env.PISPER_SMOKE_BROWSER }
    : { channel: 'msedge' }),
})
const reports = []
try {
  for (const [name, width, height, theme] of [
    ['light', 1440, 900, 'light'],
    ['dark', 1440, 900, 'dark'],
    ['mobile', 390, 844, 'light'],
    ['compact', 1100, 650, 'light'],
  ]) {
    const context = await browser.newContext({
      viewport: { width, height },
      locale: 'zh-CN',
      colorScheme: theme,
    })
    await context.addInitScript(
      ({ theme }) => {
        localStorage.setItem('pisper-language', 'zh-CN')
        localStorage.setItem('pisper-theme', theme)
        localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
      },
      { theme },
    )
    const page = await context.newPage()
    const errors = []
    page.on('pageerror', (e) => errors.push(e.message))
    await page.goto(new URL('/#/chat', base).href)
    await page.getByRole('textbox', { name: '任务描述' }).waitFor({ timeout: 60000 })
    await page.getByTestId('workbench-greeting').waitFor()
    await page.evaluate(() => document.fonts.ready)
    if (width > 650) {
      await page.getByRole('button', { name: '更多工具', exact: true }).waitFor()
      await page
        .locator('[data-slot="skeleton"]')
        .first()
        .waitFor({ state: 'hidden', timeout: 30000 })
    }
    await page.screenshot({ path: path.join(output, `${name}.png`) })
    reports.push({
      name,
      errors,
      geometry: await page.evaluate(() => ({
        width: innerWidth,
        scroll: document.documentElement.scrollWidth,
        composer: document.querySelector('.focus-composer').getBoundingClientRect().toJSON(),
      })),
    })
    assert.deepEqual(errors, [])
    const geometry = reports.at(-1).geometry
    assert(geometry.scroll <= width)
    assert(
      geometry.composer.x >= 0 &&
        geometry.composer.right <= width &&
        geometry.composer.bottom <= height,
    )
    await page.getByRole('button', { name: /模型与智力/ }).click()
    await page.getByRole('combobox', { name: '当前会话模型' }).waitFor()
    await page.getByRole('combobox', { name: '当前思考等级' }).waitFor()
    await page.keyboard.press('Escape')
    assert.equal(await page.locator('.terminal-panel:visible').count(), 0)
    await context.close()
  }
} finally {
  await browser.close()
  await writeFile(path.join(output, 'report.json'), JSON.stringify(reports, null, 2))
}
console.log(
  JSON.stringify(
    reports.map(({ name, errors, geometry }) => ({ name, errors, geometry })),
    null,
    2,
  ),
)
