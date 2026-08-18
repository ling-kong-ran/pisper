// 桌面壳桥接层（window.pisperDesktop）的类型声明：更新、目录选择、
// 语言、CLI、终端、桌面宠物、通知等能力的鸭子类型接口，供前端统一调用。
export type UpdateStatus = {
  state: string
  checkedAt?: string | null
  message?: string
  releaseUrl?: string
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
}

export type ComponentUpdateStatus = {
  component: 'desktop' | 'tui' | 'runtime'
  state: string
  currentVersion: string
  availableVersion: string
  message: string
  releaseUrl: string
  notes: string
  size: number
  transferred: number
  canInstall: boolean
  restartRequired: boolean
}

export type AppUpdateInfo = {
  desktop: boolean
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
  components: ComponentUpdateStatus[]
  check: (options?: { refresh?: boolean }) => Promise<UpdateStatus>
  installComponents: () => Promise<ComponentUpdateStatus[]>
  download: () => Promise<unknown>
  install: () => unknown
  openReleases: () => Promise<boolean>
  openUpdateLog: () => unknown
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

export type DesktopBridge = {
  platform?: string
  getAppInfo: () => Promise<AppUpdateInfo>
  pickDirectory?: (initialDirectory?: string) => Promise<string | null>
  pickFiles?: (initialDirectory?: string) => Promise<string[]>
  setLanguage?: (language: string) => Promise<string>
  getCliStatus?: () => Promise<DesktopCliStatus>
  installCli?: () => Promise<DesktopCliStatus>
  uninstallCli?: () => Promise<DesktopCliStatus>
  checkForUpdates?: () => Promise<UpdateStatus>
  downloadUpdate?: () => Promise<UpdateStatus>
  installUpdate?: () => Promise<unknown>
  componentUpdateStatus?: () => Promise<ComponentUpdateStatus[]>
  checkComponentUpdates?: () => Promise<ComponentUpdateStatus[]>
  installComponentUpdates?: () => Promise<ComponentUpdateStatus[]>
  restartForComponentUpdate?: () => Promise<unknown>
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
  openPetdex?: () => Promise<boolean>
  onUpdateStatus?: (callback: (status: UpdateStatus) => void) => () => void
}
