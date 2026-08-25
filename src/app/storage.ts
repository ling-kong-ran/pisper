// localStorage 键名统一注册表：仅收录仍由浏览器保存的 UI 状态，
// 防止各处硬编码键名漂移。命名带 pisper- 前缀避免与宿主站点冲突。
export const STORAGE_KEYS = Object.freeze({
  theme: 'pisper-theme',
  chatMode: 'pisper-chat-mode',
  activeSession: 'pisper-active-session',
  sessionOpenRequest: 'pisper-session-open-request',
  sessionCreateRequest: 'pisper-session-create-request',
  sessionMessageTarget: 'pisper-session-message-target',
  webPreviewRequest: 'pisper-web-preview-request',
  language: 'pisper-language',
  sidebarCollapsed: 'pisper-sidebar-collapsed',
  density: 'pisper-density',
  terminalPanel: 'pisper-terminal-panel',
  sponsorDismissals: 'pisper-sponsor-dismissals',
} as const)
