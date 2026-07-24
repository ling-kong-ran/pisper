/// <reference types="vite/client" />

import type { DesktopBridge } from './types/update'

declare module '*.css'

declare global {
  interface Window {
    vesperDesktop?: DesktopBridge
  }
}

export {}
