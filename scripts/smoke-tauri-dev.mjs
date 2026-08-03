import assert from 'node:assert/strict'
import process from 'node:process'
import { chromium } from 'playwright-core'

const endpoint = process.argv[2] || 'http://127.0.0.1:9223'
const deadline = Date.now() + 30_000
let browser
let pages = []

while (Date.now() < deadline) {
  try {
    browser = await chromium.connectOverCDP(endpoint)
    pages = browser.contexts().flatMap((context) => context.pages())
    if (pages.length) break
  } catch {}
  await new Promise((resolve) => setTimeout(resolve, 250))
}
assert(browser, `WebView2 debugging endpoint did not become ready: ${endpoint}`)

let mainPage
while (Date.now() < deadline && !mainPage) {
  pages = browser.contexts().flatMap((context) => context.pages())
  for (const page of pages) {
    try {
      if (await page.evaluate(() => Boolean(window.pisperDesktop?.getAppInfo))) {
        mainPage = page
        break
      }
    } catch {}
  }
  if (!mainPage) await new Promise((resolve) => setTimeout(resolve, 250))
}
assert(mainPage, 'The Tauri desktop bridge was not injected into the main WebView.')

const appInfo = await mainPage.evaluate(() => window.pisperDesktop.getAppInfo())
assert.equal(appInfo.desktop, true)
assert.equal(appInfo.version, '0.3.2')
assert.equal(appInfo.platform, 'win32')
assert.equal(appInfo.update?.state, 'idle')
assert.equal(await mainPage.evaluate(() => window.pisperDesktop.setLanguage('en-US')), 'en-US')
const update = await mainPage.evaluate(() => window.pisperDesktop.checkForUpdates())
assert.match(update.state, /^(current|available)$/)
assert.match(update.checkedAt || '', /^\d{4}-\d{2}-\d{2}T/)

const notification = await mainPage.evaluate(() => window.pisperDesktop.getNotificationStatus())
assert.equal(notification.supported, true)
assert.match(notification.permission, /^(granted|denied)$/)

const originalPet = await mainPage.evaluate(() => window.pisperDesktop.getPetStatus())
assert.equal(originalPet.supported, true)
if (!originalPet.installed.length) {
  const catalog = await mainPage.evaluate(() => window.pisperDesktop.searchPets(''))
  assert(catalog.length > 0, 'Petdex returned no installable pets.')
  await mainPage.evaluate((slug) => window.pisperDesktop.installPet(slug), catalog[0].slug)
}
const enabledPet = await mainPage.evaluate(() => window.pisperDesktop.setPetEnabled(true))
assert.equal(enabledPet.enabled, true)

let petPage
while (Date.now() < deadline && !petPage) {
  pages = browser.contexts().flatMap((context) => context.pages())
  petPage = pages.find((page) => new URL(page.url()).pathname === '/tauri-pet.html')
  if (!petPage) await new Promise((resolve) => setTimeout(resolve, 250))
}
assert(petPage, 'The independent desktop-pet WebView was not created.')
await petPage.waitForFunction(() => document.querySelector('#pet')?.classList.contains('visible'))
const sprite = await petPage.evaluate(() => {
  const element = document.querySelector('#sprite')
  const style = getComputedStyle(element)
  return {
    width: element?.clientWidth,
    height: element?.clientHeight,
    backgroundImage: style.backgroundImage,
  }
})
assert.equal(sprite.width, 192)
assert.equal(sprite.height, 208)
assert.notEqual(sprite.backgroundImage, 'none')

if (process.env.PISPER_TAURI_SMOKE_SCREENSHOT) {
  await petPage.screenshot({
    path: process.env.PISPER_TAURI_SMOKE_SCREENSHOT,
    omitBackground: true,
  })
}
await mainPage.evaluate(
  (enabled) => window.pisperDesktop.setPetEnabled(enabled),
  originalPet.enabled,
)

console.log(
  JSON.stringify({
    appInfo,
    update,
    notification,
    pet: {
      selectedSlug: enabledPet.selectedSlug,
      sprite,
    },
  }),
)
process.exit(0)
