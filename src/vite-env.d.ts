/// <reference types="vite/client" />

import type { DesktopBridge } from '@/types/update'

declare module '*.css'

declare global {
  interface Window {
    pisperDesktop?: DesktopBridge
    // Tauri 壳注入（withGlobalTauri）：移动端经代理访问桌面 UI 时可用，
    // 用于服务器切换命令与本地通知插件。
    __TAURI__?: {
      core?: { invoke?: <T = unknown>(command: string, args?: unknown) => Promise<T> }
      plugins?: {
        notification?: {
          notify?: (options: { title: string; body?: string }) => Promise<void>
          requestPermission?: () => Promise<string>
        }
      }
    }
  }
}

export {}
