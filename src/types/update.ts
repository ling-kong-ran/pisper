import type { RemoteWorkspaceBridge } from '@/types/remote-workspace'

// 桌面壳桥接层（window.pisperDesktop）的类型声明：更新、目录选择、
// 语言、CLI、终端、桌面宠物、通知等能力的鸭子类型接口，供前端统一调用。
export type UpdateStatus = {
  state: string
  checkedAt?: string | null
  message?: string
  releaseUrl?: string
  downloadUrl?: string
  canDownload?: boolean
  canInstall?: boolean
  canResume?: boolean
  notes?: string
  currentCommit?: string
  availableCommit?: string
  availableVersion?: string
  branch?: string
  percent?: number
  total?: number
  transferred?: number
  releaseDate?: string
  rateLimited?: boolean
  retryAt?: number
  // 桌面程序下好更新后怎么装：restart 由 Pisper 重启并更新；installer 是 v0.6.40 的壳，
  // 只能用系统打开安装包，由用户完成。
  installKind?: ShellInstallKind
}

export type ShellInstallKind = 'restart' | 'installer'

// 桌面程序更新（src-tauri/src/desktop_shell/shell_installer.rs）：壳在后台下载并验签整个桌面程序的
// 更新包，用户点“重启并更新”时安装。v0.6.39 及更早的壳没有这组桥接方法。
export type ShellInstallerState =
  | 'unsupported'
  | 'idle'
  | 'checking'
  | 'current'
  | 'available'
  | 'downloading'
  | 'downloaded'
  | 'error'

export type ShellInstallerStatus = {
  supported: boolean
  state: ShellInstallerState
  currentVersion: string
  availableVersion: string
  releaseUrl: string
  notes: string
  fileName: string
  size: number
  transferred: number
  canDownload: boolean
  autoDownload: boolean
  message: string
  retryAt: number | null
}

export type ShellInstallerBridge = {
  status: () => Promise<unknown>
  check: () => Promise<unknown>
  download: () => Promise<unknown>
  setAutoDownload: (enabled: boolean) => Promise<unknown>
  // 现行的壳：交给 Tauri 更新器安装后重启。
  install?: () => Promise<unknown>
  // v0.6.40 的壳：用系统打开安装包。
  open?: () => Promise<unknown>
}

export type ShellInstallerController = {
  status: ShellInstallerStatus
  installKind: ShellInstallKind
  check: () => Promise<void>
  download: () => Promise<void>
  setAutoDownload: (enabled: boolean) => Promise<void>
  install: () => Promise<boolean>
}

export type AppUpdateInfo = {
  desktop: boolean
  mobile?: boolean
  packaged: boolean
  version: string
  hostVersion?: string
  platform: string
  arch: string
  releasesUrl: string
  update?: UpdateStatus
}

export type AppUpdateController = {
  info: AppUpdateInfo
  status: UpdateStatus
  check: (options?: { refresh?: boolean }) => Promise<UpdateStatus>
  download: () => Promise<unknown>
  install: () => Promise<unknown>
  openReleases: () => Promise<boolean>
  openUpdateLog: () => unknown
  // 桌面程序更新的状态镜像：只在带 shellInstaller 桥接的壳上有值；浏览器、手机和旧壳为 null。
  shellInstaller: ShellInstallerController | null
}

export type DesktopNotificationPermission = NotificationPermission | 'checking' | 'unsupported'

export type DesktopNotificationStatus = {
  permission?: DesktopNotificationPermission
  supported?: boolean
  [key: string]: unknown
}

export type DesktopNotificationResult = {
  shown?: boolean
  permission?: DesktopNotificationPermission
  reason?: string
  [key: string]: unknown
}

export type DesktopCliStatus = {
  supported: boolean
  installed: boolean
  pathConfigured: boolean
  needsRepair: boolean
  command: string
  installPath: string
}

export type DesktopPet = {
  slug: string
  name: string
  source: 'pisper' | 'petdex'
}

export type DesktopPetCatalogItem = {
  slug: string
  displayName: string
}

export type DesktopPetStatus = {
  supported: boolean
  enabled: boolean
  running: boolean
  selectedSlug: string
  selectedName: string
  installed: DesktopPet[]
  opacity?: number
  state?: string
  stateVersion?: number
  sheetWidth?: number
  sheetHeight?: number
  spriteUrl?: string
}

export type DesktopTerminalProfile = {
  id: string
  label: string
  default: boolean
}

export type DesktopTerminalEvent =
  | { type: 'output'; terminalId: string; data: number[] }
  | { type: 'exit'; terminalId: string; code: number | null }
  | { type: 'error'; terminalId: string; message: string }

export type DesktopTerminalCreateOptions = {
  terminalId: string
  profileId: string
  cwd: string
  cols: number
  rows: number
}

export type DesktopTerminalCreated = {
  terminalId: string
  profileId: string
  cwd: string
}

// 右侧浏览器的原生子 WebView（src-tauri/src/desktop_shell/desktop_browser.rs）。
export type DesktopBrowserBounds = { x: number; y: number; width: number; height: number }
export type DesktopBrowserEvent =
  | { type: 'navigated'; url: string; loading: boolean }
  | { type: 'title'; title: string }
  | { type: 'newWindow'; url: string; browserId: string | null }
  | { type: 'closed' }
export type DesktopBrowserBridge = {
  open: (
    input: { browserId: string; url: string; bounds: DesktopBrowserBounds },
    onEvent: (event: DesktopBrowserEvent) => void,
  ) => Promise<{ status: 'opened' | 'unsupported' }>
  adopt: (
    browserId: string,
    onEvent: (event: DesktopBrowserEvent) => void,
  ) => Promise<{ status: 'opened' | 'unsupported' }>
  navigate: (browserId: string, url: string) => Promise<void>
  action: (browserId: string, action: 'back' | 'forward' | 'reload') => Promise<void>
  sync: (browserId: string, bounds: DesktopBrowserBounds, visible: boolean) => Promise<void>
  close: (browserId: string) => Promise<void>
  // Agent 接管用：在网页里执行脚本，返回 JSON 字符串。只有主窗口的界面能调，网页本身拿不到。
  evaluate: (browserId: string, script: string) => Promise<string>
  // 当前可视区域的 PNG，base64。
  capture: (browserId: string) => Promise<string>
}

// Computer Use（src-tauri/src/desktop_shell/computer_control.rs）。坐标是最近一张截图上的像素。
export type DesktopComputerAction =
  | { type: 'move'; x: number; y: number }
  | { type: 'click'; x: number; y: number; button?: 'left' | 'right'; count?: number }
  | { type: 'drag'; x: number; y: number; toX: number; toY: number }
  | { type: 'scroll'; x: number; y: number; direction: string; amount?: number }
  | { type: 'type'; text: string }
  | { type: 'key'; keys: string }
export type DesktopComputerBridge = {
  begin: (
    sessionId: string,
    labels: { title: string; hint: string; stop: string },
    onEvent: (event: { type: 'stopped' }) => void,
  ) => Promise<{
    status: 'started' | 'busy' | 'unsupported' | 'permission_required'
    reason: string | null
    missingPermissions: string[]
  }>
  end: () => Promise<void>
  screenshot: (settleMs: number) => Promise<{ imageBase64: string; width: number; height: number }>
  act: (action: DesktopComputerAction) => Promise<void>
}

export type DesktopBridge = {
  remoteWorkspaces?: RemoteWorkspaceBridge
  browser?: DesktopBrowserBridge
  computer?: DesktopComputerBridge
  customTitlebar?: boolean
  windowAction?: (action: 'minimize' | 'maximize' | 'close' | 'drag') => Promise<void>
  platform?: string
  getAppInfo: () => Promise<AppUpdateInfo>
  pickDirectory?: (initialDirectory?: string) => Promise<string | null>
  pickFiles?: (initialDirectory?: string) => Promise<string[]>
  setLanguage?: (language: string) => Promise<string>
  getCliStatus?: () => Promise<DesktopCliStatus>
  installCli?: () => Promise<DesktopCliStatus>
  uninstallCli?: () => Promise<DesktopCliStatus>
  shellInstaller?: ShellInstallerBridge
  openAsset?: (input: { name: string; data: string }) => Promise<boolean>
  openReleases: () => Promise<boolean>
  openUpdateLog?: () => Promise<unknown>
  getNotificationStatus?: () => Promise<DesktopNotificationStatus>
  openNotificationSettings?: () => Promise<boolean>
  showNotification?: (notification: {
    title: string
    body: string
  }) => Promise<DesktopNotificationResult>
  terminalProfiles?: () => Promise<DesktopTerminalProfile[]>
  terminalCreate?: (
    options: DesktopTerminalCreateOptions,
    onEvent: (event: DesktopTerminalEvent) => void,
  ) => Promise<DesktopTerminalCreated>
  terminalWrite?: (terminalId: string, data: Uint8Array) => Promise<void>
  terminalResize?: (terminalId: string, cols: number, rows: number) => Promise<void>
  terminalClose?: (terminalId: string) => Promise<boolean>
  terminalCloseAll?: () => Promise<number>
  getPetStatus?: () => Promise<DesktopPetStatus>
  setPetEnabled?: (enabled: boolean) => Promise<DesktopPetStatus>
  setPetOpacity?: (opacity: number) => Promise<DesktopPetStatus>
  searchPets?: (query: string) => Promise<DesktopPetCatalogItem[]>
  installPet?: (slug: string) => Promise<DesktopPetStatus>
  selectPet?: (slug: string) => Promise<DesktopPetStatus>
  removePet?: (slug: string) => Promise<DesktopPetStatus>
  openPetdex?: () => Promise<boolean>
  onUpdateStatus?: (callback: (status: UpdateStatus) => void) => () => void
}
