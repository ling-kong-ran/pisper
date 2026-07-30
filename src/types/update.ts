export type UpdateStatus = {
  state: string
  checkedAt?: string | null
  message?: string
  releaseUrl?: string
  canDownload?: boolean
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

export type AppUpdateInfo = {
  desktop: boolean
  packaged: boolean
  version: string
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

export type DesktopBridge = {
  platform?: string
  getAppInfo: () => Promise<AppUpdateInfo>
  pickDirectory?: (initialDirectory?: string) => Promise<string | null>
  setLanguage?: (language: string) => Promise<string>
  getCliStatus?: () => Promise<DesktopCliStatus>
  installCli?: () => Promise<DesktopCliStatus>
  uninstallCli?: () => Promise<DesktopCliStatus>
  checkForUpdates: () => Promise<UpdateStatus>
  downloadUpdate: () => Promise<UpdateStatus>
  installUpdate: () => Promise<unknown>
  openReleases: () => Promise<boolean>
  openUpdateLog?: () => Promise<unknown>
  getNotificationStatus?: () => Promise<DesktopNotificationStatus>
  openNotificationSettings?: () => Promise<boolean>
  showNotification?: (notification: {
    title: string
    body: string
  }) => Promise<DesktopNotificationResult>
  getPetStatus?: () => Promise<DesktopPetStatus>
  setPetEnabled?: (enabled: boolean) => Promise<DesktopPetStatus>
  setPetOpacity?: (opacity: number) => Promise<DesktopPetStatus>
  searchPets?: (query: string) => Promise<DesktopPetCatalogItem[]>
  installPet?: (slug: string) => Promise<DesktopPetStatus>
  selectPet?: (slug: string) => Promise<DesktopPetStatus>
  openPetdex?: () => Promise<boolean>
  onUpdateStatus: (callback: (status: UpdateStatus) => void) => () => void
}
