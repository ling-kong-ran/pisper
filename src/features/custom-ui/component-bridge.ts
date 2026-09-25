// 自定义 UI 组件的父页面桥：组件 iframe 运行在 opaque origin（sandbox 不含
// allow-same-origin），只能通过 postMessage 与应用交互。这里实现宿主侧：
// 按组件 manifest 声明的 permissions 过滤请求、代理只读 API、转发通知，
// 并在主题变化时向组件广播 CSS 变量。
import { apiJson } from '@/lib/api'
import type { CustomUiComponent } from './custom-ui-api'

// 桥协议与 runtime/services/custom-ui-service.mjs 的 BRIDGE_SCRIPT 一一对应：
// 请求 { pisperBridge: 1, id, method, params }；响应 { pisperBridge: 1, id, ok, result|error }；
// 主题推送 { pisperBridge: 1, type: 'theme', theme }。
export type PisperBridgeTheme = {
  mode: 'dark' | 'light'
  variables: Record<string, string>
  locale?: string
}

// 透传给组件的设计变量：保持与 src/index.css 的核心表面/文字/强调色一致，
// 组件按 var(--xxx) 引用即可融入当前主题（含自定义强调色）。
const THEME_VARIABLES = [
  '--bg',
  '--panel',
  '--solid',
  '--surface-subtle',
  '--surface-muted',
  '--surface-hover',
  '--stroke',
  '--stroke-soft',
  '--stroke-hover',
  '--text',
  '--text-soft',
  '--text-secondary',
  '--text-muted',
  '--brand-blue',
  '--brand-blue-soft',
  '--brand-blue-border',
  '--accent-soft',
  '--accent-strong',
  '--accent-border',
  '--star-strong',
  '--on-accent',
  '--success',
  '--success-soft',
  '--danger',
  '--danger-soft',
  '--warning-strong',
  '--warning-soft',
  '--focus',
  '--r-sm',
  '--r-md',
  '--r-lg',
] as const

export function currentBridgeTheme(): PisperBridgeTheme {
  const root = document.documentElement
  const styles = getComputedStyle(root)
  const variables: Record<string, string> = {}
  for (const name of THEME_VARIABLES) {
    const value = styles.getPropertyValue(name).trim()
    if (value) variables[name] = value
  }
  return { mode: root.classList.contains('dark') ? 'dark' : 'light', variables }
}

type BridgeRequest = {
  pisperBridge?: unknown
  id?: unknown
  method?: unknown
  params?: unknown
}

type BridgeHostOptions = {
  component: CustomUiComponent
  notify: (message: string) => void
  preview?: boolean
  locale?: string
}

// 方法 → 所需权限；ready 是握手，任何组件都可调用。
const METHOD_PERMISSIONS: Record<string, string> = {
  getConfig: 'config.read',
  listSessions: 'sessions.read',
  notify: 'notify',
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

// 绑定一个组件 iframe 的桥接宿主；返回清理函数（组件卸载/切换时调用）。
export function attachComponentBridge(
  iframe: HTMLIFrameElement,
  {
    component,
    notify,
    preview = false,
    locale = document.documentElement.lang || 'zh-CN',
  }: BridgeHostOptions,
): () => void {
  // 编辑画布只预览外观，不允许组件读取真实配置、会话或触发通知。
  const grantedPermissions = preview ? [] : component.permissions
  const permissions = new Set(grantedPermissions)
  const controller = new AbortController()
  const theme = () => ({ ...currentBridgeTheme(), locale })

  const postTheme = () => {
    if (controller.signal.aborted) return
    iframe.contentWindow?.postMessage({ pisperBridge: 1, type: 'theme', theme: theme() }, '*')
  }

  // 主题与强调色变化都体现在根元素 class / data-* 上，统一用属性观察转发。
  const themeObserver = new MutationObserver(postTheme)
  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['class', 'data-accent'],
  })

  const reply = (id: number, ok: boolean, result: unknown) => {
    if (controller.signal.aborted) return
    iframe.contentWindow?.postMessage(
      ok
        ? { pisperBridge: 1, id, ok: true, result }
        : { pisperBridge: 1, id, ok: false, error: String(result || '请求失败。') },
      '*',
    )
  }

  const handleRequest = async (method: string, params: Record<string, unknown>) => {
    if (method === 'ready') {
      // 握手响应附带主题，组件无需额外请求即可首帧正确渲染。
      return {
        component: {
          id: component.id,
          name: component.name,
          version: component.version,
          permissions: grantedPermissions,
        },
        locale,
        theme: theme(),
      }
    }
    const permission = METHOD_PERMISSIONS[method]
    if (!permission) throw new Error(`未知的桥接方法：${method}`)
    if (!permissions.has(permission)) {
      throw new Error(`组件未在 manifest.json 声明权限 ${permission}。`)
    }
    if (method === 'getConfig') return apiJson('/api/config', { signal: controller.signal })
    if (method === 'listSessions') {
      const limit = Math.min(Math.max(Number(params.limit) || 50, 1), 200)
      return apiJson(`/api/sessions?limit=${limit}`, { signal: controller.signal })
    }
    if (method === 'notify') {
      const message = String(params.message || '')
        .trim()
        .slice(0, 500)
      if (message) notify(message)
      return null
    }
    return null
  }

  const onMessage = (event: MessageEvent) => {
    // 只处理来自自己 iframe 的消息；opaque origin 下 event.origin 恒为 'null'，
    // 来源区分完全依赖 event.source 与内容Window 的引用相等性。
    if (controller.signal.aborted || event.source !== iframe.contentWindow) return
    const data: BridgeRequest = asRecord(event.data) || {}
    if (data.pisperBridge !== 1) return
    if (
      typeof data.id !== 'number' ||
      !Number.isSafeInteger(data.id) ||
      typeof data.method !== 'string'
    )
      return
    const id = data.id
    void handleRequest(data.method, asRecord(data.params) || {}).then(
      (result) => reply(id, true, result ?? null),
      (error: unknown) => reply(id, false, error instanceof Error ? error.message : String(error)),
    )
  }

  window.addEventListener('message', onMessage)
  // 重接语言或权限时同步当前外观，不必重载 iframe 丢失组件自身状态。
  postTheme()
  return () => {
    controller.abort()
    window.removeEventListener('message', onMessage)
    themeObserver.disconnect()
  }
}
