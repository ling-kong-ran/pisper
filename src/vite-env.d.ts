/// <reference types="vite/client" />

import type { DesktopBridge } from '@/types/update'

declare module '*.css'

declare global {
  interface Window {
    pisperDesktop?: DesktopBridge
    // Tauri 壳注入（withGlobalTauri）：移动端经代理访问桌面 UI 时可用，
    // 经 core.invoke 直调插件命令（通知/扫码/服务器管理）。
    __TAURI__?: {
      core?: { invoke?: <T = unknown>(command: string, args?: unknown) => Promise<T> }
    }
    // 远程代理页面上公开 global 可能被重复注入拦截，底层 IPC 入口仍可安全复用。
    __TAURI_INTERNALS__?: {
      invoke?: <T = unknown>(command: string, args?: unknown) => Promise<T>
    }
  }
}

export {}
