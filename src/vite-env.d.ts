/// <reference types="vite/client" />

import type { DesktopBridge } from '@/types/update'

declare module '*.css'

declare global {
  interface Window {
    pisperDesktop?: DesktopBridge
  }
}

export {}
