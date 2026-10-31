// 组件目录监听：<dataDir>/custom-ui/ 下任何组件的文件变化，按组件去抖 150ms 后回调 onChange(id)，
// 由 CustomUiService 重新算 rev、去重后经看板事件流发出 component_changed（规格 5.6 热更新）。
//
// - 优先 fs.watch(root, {recursive:true})，watcher 调用 unref()，不拖住进程退出；
// - 不支持递归监听（部分平台 / 手机嵌入式 Node）或监听出错时，退回每 3 秒比较一次 stat 签名；
// - 点开头的路径一律忽略：写入器的 .component-write-* / .component-old-* 暂存目录、编辑器临时文件；
//   版本快照、存储、回收站本来就在 custom-ui/ 之外，不会形成热重载循环；
// - 只由 start() 启动，导入本模块没有任何副作用；dispose() 关掉监听与全部定时器。
import { watch as fsWatch } from 'node:fs'
import { mkdir, readdir } from 'node:fs/promises'
import { join } from 'node:path'
import { COMPONENT_ID_PATTERN } from '../../shared/custom-ui-manifest.mjs'
import { listComponentFiles } from './custom-ui-files.mjs'

export const WATCH_DEBOUNCE_MS = 150
export const WATCH_POLL_MS = 3000

/**
 * 变化路径（相对 root）归属的组件 id；点开头的段或不合法的 id 返回 null。
 * @param {string | Buffer | null | undefined} filename
 * @returns {string | null}
 */
export function componentIdForPath(filename) {
  if (filename === null || filename === undefined) return null
  const segments = String(filename)
    .split(/[\\/]+/)
    .filter(Boolean)
  if (!segments.length || segments.some((segment) => segment.startsWith('.'))) return null
  return COMPONENT_ID_PATTERN.test(segments[0]) ? segments[0] : null
}

export class ComponentWatcher {
  /**
   * @param {{
   *   root: string,
   *   onChange: (id: string) => void,
   *   debounceMs?: number,
   *   pollMs?: number,
   *   watch?: typeof fsWatch,
   *   forcePoll?: boolean,
   * }} options
   */
  constructor({
    root,
    onChange,
    debounceMs = WATCH_DEBOUNCE_MS,
    pollMs = WATCH_POLL_MS,
    watch = fsWatch,
    forcePoll = false,
  }) {
    this.root = root
    this.onChange = onChange
    this.debounceMs = debounceMs
    this.pollMs = pollMs
    this.watchImpl = watch
    this.forcePoll = forcePoll
    /** @type {import('node:fs').FSWatcher | null} */
    this.watcher = null
    /** @type {ReturnType<typeof setTimeout> | null} */
    this.pollTimer = null
    /** @type {Map<string, ReturnType<typeof setTimeout>>} */
    this.pending = new Map()
    /** @type {Map<string, string> | null} */
    this.signatures = null
    this.mode = /** @type {'idle' | 'watch' | 'poll' | 'disposed'} */ ('idle')
    this.polling = false
  }

  async start() {
    if (this.mode !== 'idle') return this.mode
    await mkdir(this.root, { recursive: true, mode: 0o700 }).catch(() => {})
    if (this.mode !== 'idle') return this.mode
    if (!this.forcePoll && this.tryWatch()) return this.mode
    await this.startPolling()
    return this.mode
  }

  tryWatch() {
    try {
      const watcher = this.watchImpl(
        this.root,
        { recursive: true, persistent: false },
        (_event, filename) => {
          if (filename === null || filename === undefined) {
            // 平台没给文件名（极少见）：扫一遍签名，找出到底是哪个组件变了。
            void this.scan().catch(() => {})
            return
          }
          const id = componentIdForPath(filename)
          if (id) this.schedule(id)
        },
      )
      watcher.on('error', () => {
        // 监听失效（目录被删、句柄耗尽等）：关掉它，退回轮询，不让错误冒到进程层。
        this.closeWatcher()
        if (this.mode === 'watch') {
          this.mode = 'idle'
          void this.startPolling().catch(() => {})
        }
      })
      watcher.unref?.()
      this.watcher = watcher
      this.mode = 'watch'
      return true
    } catch {
      return false
    }
  }

  closeWatcher() {
    try {
      this.watcher?.close()
    } catch {
      // 关闭失败不影响后续轮询。
    }
    this.watcher = null
  }

  async startPolling() {
    if (this.mode === 'disposed') return
    this.mode = 'poll'
    // 第一次扫描只建立基线，不发通知（启动时没有“变化”）。
    this.signatures = await this.readSignatures().catch(() => new Map())
    this.armPoll()
  }

  armPoll() {
    if (this.mode !== 'poll') return
    this.pollTimer = setTimeout(() => {
      this.pollTimer = null
      void this.scan()
        .catch(() => {})
        .finally(() => this.armPoll())
    }, this.pollMs)
    this.pollTimer.unref?.()
  }

  /** @returns {Promise<Map<string, string>>} */
  async readSignatures() {
    /** @type {Map<string, string>} */
    const signatures = new Map()
    const entries = await readdir(this.root, { withFileTypes: true }).catch(() => [])
    for (const entry of entries) {
      if (!entry.isDirectory() || entry.name.startsWith('.')) continue
      if (!COMPONENT_ID_PATTERN.test(entry.name)) continue
      const listed = await listComponentFiles(join(this.root, entry.name))
      signatures.set(
        entry.name,
        `${listed.error || ''}|${listed.files
          .map((file) => `${file.path}:${file.size}:${file.mtimeMs}`)
          .join('|')}`,
      )
    }
    return signatures
  }

  /** 比较签名，变化、新增、消失的组件都通知。 */
  async scan() {
    if (this.polling || this.mode === 'disposed') return
    this.polling = true
    try {
      const next = await this.readSignatures()
      const previous = this.signatures || new Map()
      this.signatures = next
      // 读签名期间可能已经 dispose（await 之后状态会变，用方法读取避免类型收窄误判）。
      if (this.isDisposed()) return
      for (const [id, signature] of next) if (previous.get(id) !== signature) this.schedule(id)
      for (const id of previous.keys()) if (!next.has(id)) this.schedule(id)
    } finally {
      this.polling = false
    }
  }

  isDisposed() {
    return this.mode === 'disposed'
  }

  /** @param {string} id */
  schedule(id) {
    if (this.mode === 'disposed') return
    const existing = this.pending.get(id)
    if (existing) clearTimeout(existing)
    const timer = setTimeout(() => {
      this.pending.delete(id)
      if (this.mode === 'disposed') return
      try {
        this.onChange(id)
      } catch {
        // 通知方自己负责错误；监听不能因为一次回调失败而停下。
      }
    }, this.debounceMs)
    timer.unref?.()
    this.pending.set(id, timer)
  }

  dispose() {
    this.mode = 'disposed'
    this.closeWatcher()
    if (this.pollTimer) clearTimeout(this.pollTimer)
    this.pollTimer = null
    for (const timer of this.pending.values()) clearTimeout(timer)
    this.pending.clear()
  }
}
