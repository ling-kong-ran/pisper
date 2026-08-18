// 通知设置服务：统一管理通知渠道（浏览器/飞书/微信）的启用状态、模板与事件记录，
// 并负责把运行事件转发到各渠道。
import { randomUUID } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { sampleNotificationData } from './channels/notification-templates.mjs'

const EMPTY_BROWSER_EVENT_CURSOR = '__pisper_browser_events_empty__'

export class NotificationSettingsService {
  constructor({ path, browserEventsPath, channels }) {
    this.path = path
    this.browserEventsPath = browserEventsPath
    this.channels = channels
    this.eventWrite = Promise.resolve()
  }

  async getState() {
    const appConfig = await readJson(this.path, {})
    return {
      ...this.channels.getState(),
      browser: {
        enabled: appConfig.notifications?.browser?.enabled === true,
      },
    }
  }

  async updateBrowser(input) {
    const appConfig = await readJson(this.path, {})
    await writeJsonAtomic(this.path, {
      ...appConfig,
      notifications: {
        ...(appConfig.notifications || {}),
        browser: {
          ...(appConfig.notifications?.browser || {}),
          enabled: Boolean(input?.enabled),
        },
      },
    })
    return this.getState()
  }

  async updateTemplate(event, platform, input) {
    await this.channels.updateTemplate(event, platform, input)
    return this.getState()
  }

  testTemplate(event, platform) {
    if (platform === 'browser') return this.testBrowserTemplate(event)
    return this.channels.testNotification(event, platform)
  }

  async testBrowserTemplate(event) {
    const appConfig = await readJson(this.path, {})
    if (appConfig.notifications?.browser?.enabled !== true) throw new Error('请先启用通知。')
    const rendered = this.channels.renderNotification(event, 'browser', sampleNotificationData())
    return { sent: 1, title: rendered.title, body: rendered.content, preview: rendered.content }
  }

  async publishBrowser(title, body, event = '') {
    const appConfig = await readJson(this.path, {})
    if (appConfig.notifications?.browser?.enabled !== true || !this.browserEventsPath) return false
    const item = {
      id: randomUUID(),
      title: String(title || 'Pisper'),
      body: String(body || ''),
      event,
      createdAt: new Date().toISOString(),
    }
    this.eventWrite = this.eventWrite
      .catch(() => {})
      .then(async () => {
        const ledger = await readJson(this.browserEventsPath, { events: [] })
        ledger.events = [...(Array.isArray(ledger.events) ? ledger.events : []), item].slice(-100)
        await writeJsonAtomic(this.browserEventsPath, ledger)
      })
    await this.eventWrite
    return true
  }

  async getBrowserEvents(after = '') {
    const ledger = await readJson(this.browserEventsPath, { events: [] })
    const events = Array.isArray(ledger.events) ? ledger.events : []
    const latestId = events.at(-1)?.id || EMPTY_BROWSER_EVENT_CURSOR
    if (!after) return { events: [], latestId }
    if (after === EMPTY_BROWSER_EVENT_CURSOR) return { events: events.slice(-20), latestId }
    const index = events.findIndex((item) => item.id === after)
    return { events: index >= 0 ? events.slice(index + 1) : events.slice(-20), latestId }
  }

  async notify(event, data, { platforms, title, content } = {}) {
    const selected = new Set(platforms || ['feishu', 'weixin', 'browser'])
    const template = this.channels.getState().templates.find((item) => item.id === event)
    if (!template?.enabled) return []
    const contentOverride = typeof content === 'string' ? content.slice(0, 12_000) : undefined
    const titleOverride = typeof title === 'string' ? title.slice(0, 160) : undefined
    const results = await this.channels.notify(event, data, {
      platforms: [...selected].filter((platform) => platform !== 'browser'),
      ...(contentOverride === undefined ? {} : { content: contentOverride }),
    })
    if (selected.has('browser')) {
      const rendered = this.channels.renderNotification(event, 'browser', data)
      await this.publishBrowser(
        titleOverride || rendered.title,
        contentOverride ?? rendered.content,
        event,
      )
    }
    const failures = results.filter((result) => result.status === 'rejected')
    if (failures.length) {
      const message = failures.map((failure) => `${failure.platform}: ${failure.error}`).join('; ')
      throw new Error(`通知发送失败：${message}`)
    }
    return results
  }
}
