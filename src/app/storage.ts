// 浏览器存储键名统一注册表：仅收录仍由浏览器保存的 UI 状态，
// 防止各处硬编码键名漂移。命名带 pisper- 前缀避免与宿主站点冲突。
export const STORAGE_KEYS = Object.freeze({
  theme: 'pisper-theme',
  chatMode: 'pisper-chat-mode',
  activeSession: 'pisper-active-session',
  mobileSessionTabs: 'pisper-mobile-session-tabs',
  mobileRemoteFallbackAttempt: 'pisper-mobile-remote-fallback-attempt',
  mobileRemoteFallbackNotice: 'pisper-mobile-remote-fallback-notice',
  sessionOpenRequest: 'pisper-session-open-request',
  sessionCreateRequest: 'pisper-session-create-request',
  sessionMessageTarget: 'pisper-session-message-target',
  webPreviewRequest: 'pisper-web-preview-request',
  language: 'pisper-language',
  // 侧栏收起状态（规格 v2 2.1），唯一所有者是 src/stores/sidebar-store.ts。
  sidebar: 'pisper-sidebar',
  density: 'pisper-density',
  shortcuts: 'pisper-shortcuts',
  boardShortcuts: 'pisper-board-shortcuts',
  terminalPanel: 'pisper-terminal-panel',
  modelOnboardingDismissed: 'pisper-model-onboarding-v1-dismissed',
} as const)
