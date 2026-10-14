import { join } from 'node:path'
import {
  BrowserPreferenceError,
  parseBrowserPreferenceRevisions,
  parseBrowserPreferenceSnapshot,
  parseBrowserPreferenceUpdates,
} from '../../shared/browser-preferences.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

// Runtime 拥有跨端口的唯一偏好副本；串行写入避免相邻界面操作互相覆盖。
export class BrowserPreferencesService {
  /** @param {{ dataDir: string }} options */
  constructor({ dataDir }) {
    this.path = join(dataDir, 'pisper-browser-preferences.json')
    /** @type {Record<string, string | null>} */
    this.values = Object.create(null)
    /** @type {Record<string, number>} */
    this.revisions = Object.create(null)
    /** @type {Promise<void> | null} */
    this.initializing = null
    this.queue = Promise.resolve()
  }

  init() {
    this.initializing ??= (async () => {
      try {
        const stored = await readJson(this.path, null)
        if (stored !== null) {
          const snapshot = parseBrowserPreferenceSnapshot(stored)
          this.values = snapshot.values
          this.revisions = snapshot.revisions
        }
      } catch (error) {
        // 单独损坏的界面偏好文件不应让 Runtime 永久无法保存新的用户选择。
        if (!(error instanceof SyntaxError || error instanceof BrowserPreferenceError)) throw error
      }
    })()
    return this.initializing
  }

  async snapshot() {
    await this.init()
    await this.queue
    return { version: 1, values: { ...this.values }, revisions: { ...this.revisions } }
  }

  /** @param {unknown} input @param {unknown} [revisionsInput] */
  async update(input, revisionsInput) {
    const updates = parseBrowserPreferenceUpdates(input)
    const revisions = parseBrowserPreferenceRevisions(revisionsInput, updates, true)
    await this.init()
    const operation = this.queue.then(async () => {
      const next = { ...this.values }
      const nextRevisions = { ...this.revisions }
      let changed = false
      for (const [key, value] of Object.entries(updates)) {
        const currentRevision = nextRevisions[key] || 0
        const revision = revisions[key] || Math.max(Date.now() * 1000, currentRevision + 1)
        if (revision <= currentRevision) continue
        next[key] = value
        nextRevisions[key] = revision
        changed = true
      }
      if (!changed) return
      // 单次写入合法不代表累积快照仍合法；重启时也要能按同一上限读回。
      parseBrowserPreferenceUpdates(next)
      await writeJsonAtomic(
        this.path,
        { version: 1, values: next, revisions: nextRevisions },
        { mode: 0o600 },
      )
      this.values = next
      this.revisions = nextRevisions
    })
    this.queue = operation.catch(() => {})
    await operation
  }

  async dispose() {
    await this.initializing?.catch(() => {})
    await this.queue
  }
}
