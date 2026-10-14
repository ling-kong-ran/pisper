// 浏览器自动化服务：通过浏览器驱动执行打开页面/点击/输入/截图/提取文本等操作，
// 供 browser_automation 工具使用；所有输入（URL/选择器/尺寸）都经过安全校验。
import { access, mkdir } from 'node:fs/promises'
import { constants as fsConstants } from 'node:fs'
import { basename, join, resolve } from 'node:path'

const MAX_PAGE_TEXT_CHARS = 20_000
const MAX_ELEMENTS = 100
const DEFAULT_VIEWPORT = Object.freeze({ width: 1440, height: 900 })
const DEFAULT_IDLE_TIMEOUT_MS = 10 * 60_000

function safeDimension(value, fallback, minimum, maximum) {
  const parsed = Math.round(Number(value))
  return Number.isFinite(parsed) ? Math.min(maximum, Math.max(minimum, parsed)) : fallback
}

// URL 安全校验：仅允许 http/https。
function safeUrl(value) {
  const url = new URL(String(value || '').trim())
  if (!['http:', 'https:'].includes(url.protocol))
    throw new Error('Browser automation only supports http and https URLs.')
  return url.href
}

// 选择器安全校验：非空且限长。
function safeSelector(value) {
  const selector = String(value || '').trim()
  if (!selector) throw new Error('A selector is required for this browser action.')
  if (selector.length > 500) throw new Error('Browser selector is limited to 500 characters.')
  return selector
}

function outputName(value) {
  const requested = basename(String(value || '').trim()).replace(/[^a-zA-Z0-9._-]+/g, '-')
  const stem =
    requested.replace(/\.(?:png|jpe?g|webp)$/i, '').replace(/^-+|-+$/g, '') ||
    `browser-${Date.now()}`
  return `${stem.slice(0, 100)}.png`
}

async function executableExists(path) {
  if (!path) return false
  try {
    await access(path, fsConstants.X_OK)
    return true
  } catch {
    return false
  }
}

function pathExecutables() {
  const names =
    process.platform === 'win32'
      ? ['msedge.exe', 'chrome.exe', 'brave.exe']
      : [
          'google-chrome',
          'google-chrome-stable',
          'chromium',
          'chromium-browser',
          'microsoft-edge',
          'brave-browser',
        ]
  const directories = String(process.env.PATH || '')
    .split(process.platform === 'win32' ? ';' : ':')
    .filter(Boolean)
  return directories.flatMap((directory) => names.map((name) => join(directory, name)))
}

function browserCandidates() {
  if (process.platform === 'win32') {
    return [
      join(process.env.PROGRAMFILES || '', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
      join(
        process.env['PROGRAMFILES(X86)'] || '',
        'Microsoft',
        'Edge',
        'Application',
        'msedge.exe',
      ),
      join(process.env.PROGRAMFILES || '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
      join(process.env['PROGRAMFILES(X86)'] || '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
      join(process.env.LOCALAPPDATA || '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
      ...pathExecutables(),
    ]
  }
  if (process.platform === 'darwin') {
    return [
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
      '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
      '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser',
      ...pathExecutables(),
    ]
  }
  return pathExecutables()
}

async function inspectPage(page) {
  const snapshot = await page.evaluate(
    ({ maxElements, maxText }) => {
      const selectorFor = (element) => {
        if (element.id) return `#${CSS.escape(element.id)}`
        const name = element.getAttribute('name')
        if (name) return `${element.tagName.toLowerCase()}[name=${JSON.stringify(name)}]`
        const testId = element.getAttribute('data-testid')
        if (testId) return `[data-testid=${JSON.stringify(testId)}]`
        const aria = element.getAttribute('aria-label')
        if (aria) return `${element.tagName.toLowerCase()}[aria-label=${JSON.stringify(aria)}]`
        return element.tagName.toLowerCase()
      }
      const elements = [
        ...document.querySelectorAll(
          'a,button,input,textarea,select,[role="button"],[contenteditable="true"]',
        ),
      ]
        .filter((element) => {
          const style = getComputedStyle(element)
          const rect = element.getBoundingClientRect()
          return (
            style.visibility !== 'hidden' &&
            style.display !== 'none' &&
            rect.width > 0 &&
            rect.height > 0
          )
        })
        .slice(0, maxElements)
        .map((element) => ({
          selector: selectorFor(element),
          tag: element.tagName.toLowerCase(),
          role: element.getAttribute('role') || '',
          text: String(
            element.innerText ||
              element.value ||
              element.getAttribute('aria-label') ||
              element.getAttribute('placeholder') ||
              '',
          )
            .replace(/\s+/g, ' ')
            .trim()
            .slice(0, 240),
          href: element.href || '',
          type: element.getAttribute('type') || '',
          disabled: Boolean(element.disabled),
        }))
      return {
        title: document.title,
        url: location.href,
        text: String(document.body?.innerText || '').slice(0, maxText),
        elements,
      }
    },
    { maxElements: MAX_ELEMENTS, maxText: MAX_PAGE_TEXT_CHARS },
  )
  return snapshot
}

export class BrowserAutomationService {
  constructor({
    driver,
    idleTimeoutMs = DEFAULT_IDLE_TIMEOUT_MS,
    setTimer = setTimeout,
    clearTimer = clearTimeout,
  } = {}) {
    this.driver = driver || null
    this.sessions = new Map()
    this.idleTimers = new Map()
    this.idleTimeoutMs = Math.max(0, Number(idleTimeoutMs) || 0)
    this.setTimer = setTimer
    this.clearTimer = clearTimer
  }

  clearIdleTimer(sessionId) {
    const id = String(sessionId || 'default')
    if (!this.idleTimers.has(id)) return
    const timer = this.idleTimers.get(id)
    this.idleTimers.delete(id)
    this.clearTimer(timer)
  }

  touchSession(sessionId) {
    const id = String(sessionId || 'default')
    this.clearIdleTimer(id)
    if (!this.idleTimeoutMs) return
    const timer = this.setTimer(() => {
      if (this.idleTimers.get(id) !== timer) return
      this.idleTimers.delete(id)
      void this.closeSession(id).catch(() => {})
    }, this.idleTimeoutMs)
    timer?.unref?.()
    this.idleTimers.set(id, timer)
  }

  async launchPlaywright(viewport) {
    const { chromium } = await import('playwright-core')
    let lastError = null
    for (const executablePath of [...new Set(browserCandidates())]) {
      if (!(await executableExists(executablePath))) continue
      try {
        const browser = await chromium.launch({ executablePath, headless: true })
        const context = await browser.newContext({ viewport, acceptDownloads: false })
        const page = await context.newPage()
        page.setDefaultTimeout(15_000)
        page.setDefaultNavigationTimeout(30_000)
        return { browser, context, page, executablePath }
      } catch (error) {
        lastError = error
      }
    }
    throw new Error(
      `No controllable browser was found. Install Chrome, Edge, Chromium, or run Pisper Desktop.${lastError ? ` ${lastError.message}` : ''}`,
    )
  }

  async ensureSession(sessionId, viewport) {
    const id = String(sessionId || 'default')
    if (this.driver) return { id, driver: true }
    let current = this.sessions.get(id)
    if (!current) {
      current = await this.launchPlaywright(viewport)
      this.sessions.set(id, current)
    }
    return current
  }

  async execute(sessionId, input = {}, { cwd, signal, onProgress } = {}) {
    const action = String(input.action || 'inspect')
    const id = String(sessionId || 'default')
    const viewport = {
      width: safeDimension(input.width, DEFAULT_VIEWPORT.width, 640, 2560),
      height: safeDimension(input.height, DEFAULT_VIEWPORT.height, 480, 1600),
    }
    signal?.throwIfAborted?.()
    if (action === 'close') this.clearIdleTimer(id)
    else this.touchSession(id)
    if (this.driver) {
      const outputPath =
        action === 'screenshot' ? await this.screenshotPath(cwd, input.outputName) : undefined
      return this.driver.execute(
        id,
        { ...input, action, viewport, outputPath },
        { signal, onProgress },
      )
    }
    if (action === 'close') {
      await this.closeSession(sessionId)
      return { action, closed: true }
    }
    const current = await this.ensureSession(sessionId, viewport)
    const { page } = current
    if (action === 'open') {
      const url = safeUrl(input.url)
      onProgress?.(`Opening ${url}`)
      await page.setViewportSize(viewport)
      await page.goto(url, { waitUntil: 'domcontentloaded' })
      if (input.waitMs)
        await page.waitForTimeout(Math.min(15_000, Math.max(0, Number(input.waitMs) || 0)))
      return { action, url: page.url(), title: await page.title(), viewport }
    }
    if (action === 'inspect') return { action, ...(await inspectPage(page)) }
    if (action === 'click') {
      const selector = safeSelector(input.selector)
      onProgress?.(`Clicking ${selector}`)
      await page.locator(selector).first().click()
      if (input.waitMs)
        await page.waitForTimeout(Math.min(15_000, Math.max(0, Number(input.waitMs) || 0)))
      return { action, selector, url: page.url(), title: await page.title() }
    }
    if (action === 'type') {
      const selector = safeSelector(input.selector)
      const text = String(input.text || '').slice(0, 5_000)
      onProgress?.(`Typing into ${selector}`)
      await page.locator(selector).first().fill(text)
      if (input.submit) await page.locator(selector).first().press('Enter')
      if (input.waitMs)
        await page.waitForTimeout(Math.min(15_000, Math.max(0, Number(input.waitMs) || 0)))
      return {
        action,
        selector,
        submitted: Boolean(input.submit),
        url: page.url(),
        title: await page.title(),
      }
    }
    if (action === 'wait') {
      const waitMs = Math.min(15_000, Math.max(0, Number(input.waitMs) || 1_000))
      await page.waitForTimeout(waitMs)
      return { action, waitMs, url: page.url(), title: await page.title() }
    }
    if (action === 'screenshot') {
      const path = await this.screenshotPath(cwd, input.outputName)
      onProgress?.(`Capturing ${page.url()}`)
      await page.screenshot({ path, fullPage: input.fullPage !== false, type: 'png' })
      return {
        action,
        path,
        name: basename(path),
        mimeType: 'image/png',
        url: page.url(),
        title: await page.title(),
        fullPage: input.fullPage !== false,
        viewport,
      }
    }
    throw new Error(`Unsupported browser action: ${action}`)
  }

  async screenshotPath(cwd, name) {
    const directory = resolve(cwd || process.cwd(), 'generated', 'browser')
    await mkdir(directory, { recursive: true })
    return join(directory, outputName(name))
  }

  async closeSession(sessionId) {
    const id = String(sessionId || 'default')
    this.clearIdleTimer(id)
    if (this.driver) return this.driver.closeSession?.(id)
    const current = this.sessions.get(id)
    if (!current) return false
    this.sessions.delete(id)
    await current.context?.close().catch(() => {})
    await current.browser?.close().catch(() => {})
    return true
  }

  async dispose() {
    for (const id of [...this.idleTimers.keys()]) this.clearIdleTimer(id)
    if (this.driver) return this.driver.dispose?.()
    await Promise.all([...this.sessions.keys()].map((id) => this.closeSession(id)))
  }
}
