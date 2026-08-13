import { chromium } from 'playwright-core'

function launchOptions() {
  return {
    headless: process.env.SCREENSHOT_HEADLESS !== '0',
  }
}

export async function launchScreenshotBrowser() {
  const executablePath = String(process.env.SCREENSHOT_BROWSER_PATH || '').trim()
  if (executablePath) {
    return chromium.launch({ ...launchOptions(), executablePath })
  }

  const configuredChannel = String(process.env.SCREENSHOT_BROWSER_CHANNEL || '').trim()
  const channels = configuredChannel ? [configuredChannel] : ['msedge', 'chrome']
  const failures = []
  for (const channel of channels) {
    try {
      return await chromium.launch({ ...launchOptions(), channel })
    } catch (error) {
      failures.push(`${channel}: ${error instanceof Error ? error.message : String(error)}`)
    }
  }

  try {
    return await chromium.launch(launchOptions())
  } catch (error) {
    failures.push(`bundled chromium: ${error instanceof Error ? error.message : String(error)}`)
  }

  throw new Error(
    `Unable to launch a Chromium browser. Set SCREENSHOT_BROWSER_PATH or ` +
      `SCREENSHOT_BROWSER_CHANNEL.\n${failures.join('\n')}`,
  )
}
