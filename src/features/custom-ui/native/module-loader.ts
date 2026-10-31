// 组件模块加载（规格 5.6、计划 §3.6 / §4.3）：import('/api/custom-ui/components/<id>/r/<rev>/<entry>')。
// - 按 `${id}@${rev}` 缓存 Promise：同一版本的多个实例共用一次导入，热重载换 rev 才重新导入。
// - 浏览器不会回收已导入的模块：每个组件记录成功导入过的版本数，≥20 时提示刷新页面释放旧版本。
// - 失败的导入不缓存（网络抖动后 [重新加载] 能重试）；语法错误等同一内容的失败只能靠新 rev 修复。
// importer 可注入，测试不需要真的发起动态导入。
import { componentModuleUrl } from '@/features/custom-ui/custom-ui-api'

export const MODULE_VERSION_LIMIT = 20

export type ComponentFunction = (input: {
  props: Record<string, unknown>
  size: unknown
  placement: unknown
  ctx: unknown
}) => unknown

export type ComponentLoadErrorCode = 'module_load_failed' | 'default_export_invalid'

/** 加载失败（404、语法错误、default 不是函数）；宿主据此显示 kind:'load' 的错误卡。 */
export class ComponentLoadError extends Error {
  code: ComponentLoadErrorCode
  constructor(code: ComponentLoadErrorCode, message: string, options?: { cause?: unknown }) {
    super(message, options)
    this.name = 'ComponentLoadError'
    this.code = code
  }
}

export type ModuleImporter = (url: string) => Promise<unknown>

export type ModuleLoader = {
  load(id: string, rev: string, entry: string): Promise<ComponentFunction>
  // 已成功导入的版本数与是否建议刷新页面（工作室状态栏用）。
  versions(id: string): number
  reloadRecommended(id: string): boolean
  subscribe(listener: () => void): () => void
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export function createModuleLoader({
  importer,
  limit = MODULE_VERSION_LIMIT,
}: {
  importer: ModuleImporter
  limit?: number
}): ModuleLoader {
  const cache = new Map<string, Promise<ComponentFunction>>()
  const imported = new Map<string, Set<string>>()
  const listeners = new Set<() => void>()
  return {
    load(id, rev, entry) {
      const key = `${id}@${rev}`
      const cached = cache.get(key)
      if (cached) return cached
      let url: string
      try {
        url = componentModuleUrl(id, rev, entry)
      } catch (error) {
        return Promise.reject(
          new ComponentLoadError('module_load_failed', errorText(error), { cause: error }),
        )
      }
      const pending = importer(url).then(
        (module) => {
          const candidate =
            module && typeof module === 'object' ? (module as { default?: unknown }).default : null
          if (typeof candidate !== 'function')
            throw new ComponentLoadError(
              'default_export_invalid',
              'Component module must export a default function',
            )
          let revisions = imported.get(id)
          if (!revisions) imported.set(id, (revisions = new Set()))
          if (!revisions.has(rev)) {
            revisions.add(rev)
            for (const listener of [...listeners]) listener()
          }
          return candidate as ComponentFunction
        },
        (error: unknown) => {
          throw error instanceof ComponentLoadError
            ? error
            : new ComponentLoadError('module_load_failed', errorText(error), { cause: error })
        },
      )
      cache.set(key, pending)
      pending.catch(() => {
        if (cache.get(key) === pending) cache.delete(key)
      })
      return pending
    },
    versions: (id) => imported.get(id)?.size ?? 0,
    reloadRecommended: (id) => (imported.get(id)?.size ?? 0) >= limit,
    subscribe(listener) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
  }
}

/**
 * 组件样式（manifest.styles）以 <link> 注入 <head>，同一 href 按引用计数共享；最后一个实例卸载时撤下。
 * 换 rev 时 href 跟着变，旧版本的样式随旧实例卸载一起撤下。
 */
export function createStyleRegistry(doc: Pick<Document, 'createElement' | 'head'>) {
  const links = new Map<string, { element: HTMLLinkElement; count: number }>()
  return {
    retain(id: string, rev: string, styles: readonly string[]): () => void {
      const hrefs: string[] = []
      for (const style of styles) {
        let href: string
        try {
          href = componentModuleUrl(id, rev, style)
        } catch {
          continue
        }
        hrefs.push(href)
        const existing = links.get(href)
        if (existing) {
          existing.count += 1
          continue
        }
        const element = doc.createElement('link')
        element.rel = 'stylesheet'
        element.href = href
        element.dataset.componentStyle = id
        doc.head.appendChild(element)
        links.set(href, { element, count: 1 })
      }
      let released = false
      return () => {
        if (released) return
        released = true
        for (const href of hrefs) {
          const entry = links.get(href)
          if (!entry) continue
          entry.count -= 1
          if (entry.count > 0) continue
          entry.element.remove()
          links.delete(href)
        }
      }
    },
    size: () => links.size,
  }
}
