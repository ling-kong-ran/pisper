import { existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, statSync, unlinkSync, writeFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { isAbsolute, join, relative } from 'node:path'
import { app, autoUpdater as nativeAutoUpdater, BrowserWindow, dialog, ipcMain, Menu, nativeImage, nativeTheme, net, Notification as ElectronNotification, screen, shell, Tray } from 'electron'
import updater from 'electron-updater'
import { createVesperServer } from '../server/app-server.mjs'
import { createElectronBrowserAutomationDriver } from './browser-automation.mjs'
import { getDesktopNotificationStatus, WINDOWS_NOTIFICATION_SETTINGS_URL } from './desktop-notifications.mjs'
import { getDesktopLanguage, isSupportedLanguage, setDesktopLanguage, t } from './i18n.mjs'
import { enableResumableUpdateDownloads } from './resumable-update-download.mjs'
import { fetchAllowedHttps } from './petdex-fetch.mjs'
import { createUpdateLogger, shutdownWithDeadline } from './update-lifecycle.mjs'
import { LATEST_RELEASE_API, newerVersion, normalizedVersion, reconcileDesktopUpdateCheck, RELEASES_URL } from '../shared/app-update.mjs'
import { releaseNotesMarkdown } from '../shared/release-notes.mjs'
import {
  MAX_PET_BYTES,
  PET_WINDOW_HEIGHT,
  PET_WINDOW_WIDTH,
  PETDEX_PAGE_URL,
  isPetSheetDimensions,
  petBubbleKeyForState,
  petStateForAgentEvent,
  readImageDimensions,
  resolvePetPosition,
} from './desktop-pet-state.mjs'

const { autoUpdater } = updater
const UPDATE_CHANNEL = 'vesper:update-status'
const APP_USER_MODEL_ID = 'com.lingkongran.vesper'
const CLOSE_ACTION_ASK = 'ask'
const CLOSE_ACTION_TRAY = 'tray'
const CLOSE_ACTION_QUIT = 'quit'
const PETDEX_MANIFEST_URL = 'https://petdex.dev/api/manifest'
const NO_CACHE_HEADERS = Object.freeze({
  'Cache-Control': 'no-cache',
  Pragma: 'no-cache',
})
let mainWindow = null
let petWindow = null
let tray = null
let vesperServer = null
let updateCheck = null
let quitting = false
let closePromptOpen = false
let updateState = { state: 'idle', checkedAt: null }
let updateLogger = console
let updateLogPath = ''
let installingUpdate = false
let petState = 'idle'
let petStateResetTimer = null
let petWindowCreation = null
let petdexManifestCache = null
let petdexManifestExpiresAt = 0
let petDragStart = null
let petClosing = false
const activePetSessions = new Set()
const activeDesktopNotifications = new Set()

process.env.PI_SKIP_VERSION_CHECK ||= '1'
process.env.PI_TELEMETRY ||= '0'

function emitUpdateState() {
  for (const window of BrowserWindow.getAllWindows()) window.webContents.send(UPDATE_CHANNEL, updateState)
  return updateState
}

function publishUpdate(patch) {
  updateState = { ...updateState, ...patch }
  return emitUpdateState()
}

function beginUpdateCheck() {
  // Full replace so a previous 0.1.x result cannot linger while checking for 0.2.x.
  updateState = {
    state: 'checking',
    message: '',
    availableVersion: '',
    releaseDate: null,
    notes: '',
    releaseUrl: RELEASES_URL,
    canDownload: false,
    canInstall: false,
    canResume: false,
    percent: 0,
    bytesPerSecond: 0,
    transferred: 0,
    total: 0,
    checkedAt: null,
  }
  return emitUpdateState()
}

async function githubLatestRelease() {
  const response = await net.fetch(`${LATEST_RELEASE_API}?_=${Date.now()}`, {
    headers: {
      Accept: 'application/vnd.github+json',
      'X-GitHub-Api-Version': '2022-11-28',
      'User-Agent': `Vesper/${app.getVersion()}`,
      ...NO_CACHE_HEADERS,
    },
  })
  if (!response.ok) throw new Error(t('update.githubReleaseRequestFailed', { status: response.status }))
  const release = await response.json()
  const version = normalizedVersion(release.tag_name)
  return {
    version,
    releaseDate: release.published_at || release.created_at || null,
    notes: releaseNotesMarkdown(release.body),
    releaseUrl: release.html_url || RELEASES_URL,
    available: newerVersion(version, app.getVersion()),
  }
}

function invalidateUpdaterMetadata() {
  autoUpdater.requestHeaders = { ...NO_CACHE_HEADERS }
  // Drop provider/client cache so the next check re-reads GitHub channel files.
  autoUpdater.clientPromise = null
  autoUpdater.updateInfoAndProvider = null
}

async function checkForUpdates({ silent = false } = {}) {
  if (updateCheck) return updateCheck
  if (!silent) beginUpdateCheck()
  else if (updateState.state === 'idle') publishUpdate({ state: 'checking', message: '' })

  updateCheck = (async () => {
    try {
      const latest = await githubLatestRelease()
      if (!app.isPackaged) {
        return publishUpdate({
          ...reconcileDesktopUpdateCheck({
            appVersion: app.getVersion(),
            githubVersion: latest.version,
            githubReleaseDate: latest.releaseDate,
            githubNotes: latest.notes,
            githubReleaseUrl: latest.releaseUrl,
          }),
          canDownload: false,
          checkedAt: new Date().toISOString(),
          message: latest.available
            ? t('update.devModeCheckOnly')
            : t('update.upToDate'),
        })
      }

      invalidateUpdaterMetadata()
      let result = await autoUpdater.checkForUpdates()
      let updaterVersion = normalizedVersion(result?.updateInfo?.version || updateState.availableVersion || '')

      // GitHub API is authoritative; if the channel metadata lags, force one more lookup.
      if (latest.version && updaterVersion && newerVersion(latest.version, updaterVersion)) {
        updateLogger.warn('Update channel metadata lagged behind GitHub Releases; rechecking.', {
          githubVersion: latest.version,
          updaterVersion,
        })
        invalidateUpdaterMetadata()
        result = await autoUpdater.checkForUpdates()
        updaterVersion = normalizedVersion(result?.updateInfo?.version || updateState.availableVersion || '')
      }

      const reconciled = reconcileDesktopUpdateCheck({
        appVersion: app.getVersion(),
        githubVersion: latest.version,
        githubReleaseDate: latest.releaseDate,
        githubNotes: latest.notes || updateState.notes || '',
        githubReleaseUrl: latest.releaseUrl,
        updaterVersion,
        updaterIsAvailable: Boolean(result?.isUpdateAvailable),
        previousState: updateState.state,
        previousAvailableVersion: updateState.availableVersion,
      })

      return publishUpdate({
        ...reconciled,
        // Keep live download metrics if a matching download is already in progress.
        ...(updateState.state === 'downloading' && updateState.availableVersion === reconciled.availableVersion
          ? {
              state: 'downloading',
              percent: updateState.percent,
              bytesPerSecond: updateState.bytesPerSecond,
              transferred: updateState.transferred,
              total: updateState.total,
              canDownload: false,
              canResume: false,
              message: '',
            }
          : {}),
        checkedAt: new Date().toISOString(),
      })
    } catch (error) {
      return publishUpdate({
        state: 'error',
        message: error instanceof Error ? error.message : String(error),
        canResume: false,
        canDownload: false,
        canInstall: false,
        checkedAt: new Date().toISOString(),
      })
    } finally {
      updateCheck = null
    }
  })()
  return updateCheck
}

function configureUpdater() {
  updateLogPath = join(app.getPath('logs'), 'updater.log')
  updateLogger = createUpdateLogger({ filePath: updateLogPath })
  autoUpdater.logger = updateLogger
  enableResumableUpdateDownloads(autoUpdater, { logger: updateLogger })
  autoUpdater.autoDownload = false
  autoUpdater.autoInstallOnAppQuit = false
  autoUpdater.autoRunAppAfterInstall = true
  autoUpdater.allowPrerelease = false
  autoUpdater.requestHeaders = { ...NO_CACHE_HEADERS }
  updateLogger.info('Updater initialized.', { version: app.getVersion(), packaged: app.isPackaged, executable: app.getPath('exe') })
  nativeAutoUpdater?.on?.('before-quit-for-update', () => updateLogger.info('Electron requested application quit for an update.'))
  autoUpdater.on('checking-for-update', () => {
    updateLogger.info('Checking for updates.')
    publishUpdate({ state: 'checking', message: '' })
  })
  autoUpdater.on('update-available', (info) => {
    updateLogger.info('Update available.', { version: info.version, releaseDate: info.releaseDate })
    publishUpdate({
      state: 'available',
      availableVersion: info.version,
      releaseDate: info.releaseDate || null,
      notes: releaseNotesMarkdown(info.releaseNotes),
      releaseUrl: RELEASES_URL,
      canDownload: true,
      canResume: false,
      checkedAt: new Date().toISOString(),
      message: '',
    })
  })
  autoUpdater.on('update-not-available', (info) => {
    updateLogger.info('No update available.', { version: info.version || app.getVersion() })
    publishUpdate({
      state: 'current',
      availableVersion: info.version || app.getVersion(),
      releaseDate: info.releaseDate || null,
      notes: releaseNotesMarkdown(info.releaseNotes),
      releaseUrl: RELEASES_URL,
      canDownload: false,
      canResume: false,
      checkedAt: new Date().toISOString(),
      message: t('update.upToDate'),
    })
  })
  autoUpdater.on('download-progress', (progress) => publishUpdate({
    state: 'downloading',
    percent: Math.max(0, Math.min(100, Number(progress.percent) || 0)),
    bytesPerSecond: Number(progress.bytesPerSecond) || 0,
    transferred: Number(progress.transferred) || 0,
    total: Number(progress.total) || 0,
    message: '',
  }))
  autoUpdater.on('update-downloaded', (info) => {
    updateLogger.info('Update downloaded and ready to install.', { version: info.version, downloadedFile: info.downloadedFile || '' })
    publishUpdate({
      state: 'downloaded',
      availableVersion: info.version,
      releaseDate: info.releaseDate || updateState.releaseDate || null,
      notes: releaseNotesMarkdown(info.releaseNotes) || updateState.notes || '',
      releaseUrl: RELEASES_URL,
      canDownload: false,
      canInstall: true,
      canResume: false,
      percent: 100,
      message: t('update.downloadedRestartToInstall'),
    })
  })
  autoUpdater.on('error', (error) => {
    updateLogger.error('Updater error.', error)
    const canResume = updateState.state === 'downloading' && Boolean(updateState.availableVersion && updateState.canDownload)
    publishUpdate({
      state: 'error',
      message: error instanceof Error ? error.message : String(error),
      canResume,
      checkedAt: new Date().toISOString(),
    })
  })
}

async function prepareApplicationShutdown({ exit = true } = {}) {
  const server = vesperServer
  vesperServer = null
  updateLogger.info('Application shutdown started.', { reason: installingUpdate ? 'update' : 'quit' })
  return shutdownWithDeadline({
    destroy: () => {
      if (tray) {
        tray.destroy()
        tray = null
      }
      destroyDesktopPet()
      if (mainWindow && !mainWindow.isDestroyed()) mainWindow.destroy()
      mainWindow = null
    },
    close: () => server?.close(),
    ...(exit ? { exit: (code) => app.exit(code) } : {}),
    logger: updateLogger,
  })
}

function resolveAppIconPath(appRoot = app.getAppPath()) {
  const icon = join(appRoot, 'build', 'icon.png')
  return existsSync(icon) ? icon : null
}

function resolveTrayIconPath(appRoot = app.getAppPath()) {
  const candidates = process.platform === 'win32'
    ? ['build/icons/16x16.png', 'build/icons/32x32.png', 'build/icon.png']
    : process.platform === 'darwin'
      ? ['build/icons/16x16.png', 'build/icons/32x32.png', 'build/icon.png']
      : ['build/icons/32x32.png', 'build/icons/24x24.png', 'build/icon.png']
  for (const relative of candidates) {
    const fullPath = join(appRoot, relative)
    if (existsSync(fullPath)) return fullPath
  }
  return resolveAppIconPath(appRoot)
}

async function showMainWindow() {
  if (!mainWindow || mainWindow.isDestroyed()) {
    await createWindow()
    return
  }
  if (mainWindow.isMinimized()) mainWindow.restore()
  mainWindow.show()
  if (typeof mainWindow.moveTop === 'function') mainWindow.moveTop()
  mainWindow.focus()
}

function hideMainWindowToTray() {
  if (!mainWindow || mainWindow.isDestroyed()) return
  mainWindow.hide()
}

const PET_SPRITE_NAMES = Object.freeze([
  'spritesheet.webp',
  'spritesheet.png',
  'sprite.webp',
  'sprite.png',
])

function managedDesktopPetRoot() {
  return join(app.getPath('userData'), 'desktop-pets')
}

function desktopPetRoots() {
  return [managedDesktopPetRoot(), join(homedir(), '.petdex', 'pets'), join(homedir(), '.codex', 'pets')]
}

function activeDesktopPetSlug() {
  try {
    const data = JSON.parse(readFileSync(join(homedir(), '.petdex', 'active.json'), 'utf8'))
    return typeof data?.slug === 'string' ? data.slug : ''
  } catch {
    return ''
  }
}

function loadInstalledDesktopPet(root, slug) {
  let petDirectory
  try {
    const entry = readdirSync(root, { withFileTypes: true }).find((item) => item.isDirectory() && item.name === slug)
    if (!entry) return null
    petDirectory = join(root, entry.name)
  } catch {
    return null
  }

  for (const fileName of PET_SPRITE_NAMES) {
    const spritePath = join(petDirectory, fileName)
    try {
      const relativePath = relative(realpathSync(root), realpathSync(spritePath))
      if (!relativePath || relativePath.startsWith('..') || isAbsolute(relativePath)) continue
      const size = statSync(spritePath).size
      if (size <= 0 || size > MAX_PET_BYTES) continue
      const buffer = readFileSync(spritePath)
      const image = readImageDimensions(buffer)
      if (!image || !isPetSheetDimensions(image)) continue
      let name = slug
      try {
        const metadata = JSON.parse(readFileSync(join(petDirectory, 'pet.json'), 'utf8'))
        name = String(metadata?.displayName || metadata?.name || slug).trim() || slug
      } catch {
        // pet.json is optional in the upstream desktop loader.
      }
      return { slug, name, buffer, ...image }
    } catch {
      // Try the next supported sprite file name.
    }
  }
  return null
}

function findInstalledDesktopPet(slug) {
  if (!slug) return null
  for (const root of desktopPetRoots()) {
    const pet = loadInstalledDesktopPet(root, slug)
    if (pet) return pet
  }
  return null
}

function resolveInstalledDesktopPet(preferredSlug = '') {
  const roots = desktopPetRoots()
  const preferred = [preferredSlug, activeDesktopPetSlug()].filter(Boolean)
  for (const slug of preferred) {
    const pet = findInstalledDesktopPet(slug)
    if (pet) return pet
  }
  for (const root of roots) {
    let slugs = []
    try {
      slugs = readdirSync(root, { withFileTypes: true })
        .filter((entry) => entry.isDirectory())
        .map((entry) => entry.name)
        .sort()
    } catch {
      continue
    }
    for (const slug of slugs) {
      const pet = loadInstalledDesktopPet(root, slug)
      if (pet) return pet
    }
  }
  throw new Error(t('pet.noInstalledPet'))
}

function installedDesktopPets() {
  const managedRoot = managedDesktopPetRoot()
  const pets = []
  const seen = new Set()
  for (const root of desktopPetRoots()) {
    let slugs = []
    try {
      slugs = readdirSync(root, { withFileTypes: true })
        .filter((entry) => entry.isDirectory())
        .map((entry) => entry.name)
        .sort()
    } catch {
      continue
    }
    for (const slug of slugs) {
      if (seen.has(slug)) continue
      const pet = loadInstalledDesktopPet(root, slug)
      if (!pet) continue
      seen.add(slug)
      pets.push({ slug: pet.slug, name: pet.name, source: root === managedRoot ? 'vesper' : 'petdex' })
    }
  }
  return pets
}

function desktopPetStatus() {
  const preferences = loadDesktopPreferences()
  const installed = installedDesktopPets()
  const selected = installed.find((pet) => pet.slug === preferences.petSlug) || installed[0] || null
  return {
    supported: true,
    enabled: preferences.petEnabled,
    running: Boolean(petWindow && !petWindow.isDestroyed() && petWindow.isVisible()),
    selectedSlug: selected?.slug || '',
    selectedName: selected?.name || '',
    installed,
  }
}

async function boundedPetdexFetch(value, maxBytes, allowedHost, redirectHosts = []) {
  let response
  try {
    const result = await fetchAllowedHttps(globalThis.fetch, value, {
      allowedHost,
      redirectHosts,
      headers: { 'User-Agent': `Vesper/${app.getVersion()}` },
    })
    response = result.response
  } catch (error) {
    if (error instanceof Error && error.message === 'UNTRUSTED_URL')
      throw new Error(t('pet.untrustedAsset'))
    throw error
  }
  if (!response.ok) throw new Error(`Petdex request failed: HTTP ${response.status}`)
  const declaredSize = Number(response.headers.get('content-length') || 0)
  if (declaredSize > maxBytes) throw new Error(t('pet.assetTooLarge'))
  const buffer = Buffer.from(await response.arrayBuffer())
  if (!buffer.length || buffer.length > maxBytes) throw new Error(t('pet.assetTooLarge'))
  return { buffer, contentType: String(response.headers.get('content-type') || '').toLowerCase() }
}

async function petdexManifest() {
  if (petdexManifestCache && Date.now() < petdexManifestExpiresAt) return petdexManifestCache
  const { buffer } = await boundedPetdexFetch(
    PETDEX_MANIFEST_URL,
    5 * 1024 * 1024,
    'petdex.dev',
    ['assets.petdex.dev'],
  )
  const data = JSON.parse(buffer.toString('utf8'))
  const pets = Array.isArray(data?.pets) ? data.pets : []
  petdexManifestCache = pets.slice(0, 5000).flatMap((pet) => {
    const slug = String(pet?.slug || '').trim()
    const displayName = String(pet?.displayName || slug).trim()
    const spritesheetUrl = String(pet?.spritesheetUrl || '')
    if (!/^[a-z0-9][a-z0-9-]{0,79}$/.test(slug)) return []
    try {
      const asset = new URL(spritesheetUrl)
      if (asset.protocol !== 'https:' || asset.hostname !== 'assets.petdex.dev') return []
    } catch {
      return []
    }
    return [{ slug, displayName: displayName || slug, spritesheetUrl }]
  })
  petdexManifestExpiresAt = Date.now() + 5 * 60 * 1000
  return petdexManifestCache
}

async function installManagedDesktopPet(inputSlug) {
  const slug = String(inputSlug || '').trim().toLowerCase()
  if (!/^[a-z0-9][a-z0-9-]{0,79}$/.test(slug)) throw new Error(t('pet.invalidSlug'))
  const manifest = await petdexManifest()
  const entry = manifest.find((pet) => pet.slug === slug)
  if (!entry) throw new Error(t('pet.notFound'))
  const { buffer, contentType } = await boundedPetdexFetch(entry.spritesheetUrl, MAX_PET_BYTES, 'assets.petdex.dev')
  const image = readImageDimensions(buffer)
  if (!image || !isPetSheetDimensions(image)) throw new Error(t('pet.invalidSheet'))
  if (contentType && !contentType.startsWith(image.mime)) throw new Error(t('pet.invalidSheet'))
  const directory = join(managedDesktopPetRoot(), slug)
  mkdirSync(directory, { recursive: true })
  for (const fileName of PET_SPRITE_NAMES) {
    try {
      unlinkSync(join(directory, fileName))
    } catch {
      // Missing previous formats are expected.
    }
  }
  const extension = image.mime === 'image/png' ? 'png' : 'webp'
  writeFileSync(join(directory, `spritesheet.${extension}`), buffer)
  writeFileSync(join(directory, 'pet.json'), `${JSON.stringify({ id: slug, displayName: entry.displayName }, null, 2)}\n`, 'utf8')
  saveDesktopPreferences({ petSlug: slug })
  if (loadDesktopPreferences().petEnabled) {
    destroyDesktopPet()
    await createDesktopPet({ notifyOnError: true })
  }
  return desktopPetStatus()
}

async function selectDesktopPet(slug) {
  const pet = findInstalledDesktopPet(String(slug || ''))
  if (!pet) throw new Error(t('pet.notInstalled'))
  saveDesktopPreferences({ petSlug: pet.slug })
  if (loadDesktopPreferences().petEnabled) {
    destroyDesktopPet()
    await createDesktopPet({ notifyOnError: true })
  }
  return desktopPetStatus()
}

function petStatePayload(state = petState) {
  const bubbleKey = petBubbleKeyForState(state)
  return { state, bubble: bubbleKey ? t(bubbleKey) : '' }
}

function sendPetState() {
  if (!petWindow || petWindow.isDestroyed() || petWindow.webContents.isLoading()) return
  petWindow.webContents.send('vesper:pet-state', petStatePayload())
}

function publishPetState(state, { resetAfter = 0 } = {}) {
  if (petStateResetTimer) clearTimeout(petStateResetTimer)
  petStateResetTimer = null
  petState = state
  sendPetState()
  if (resetAfter > 0) {
    petStateResetTimer = setTimeout(() => {
      petStateResetTimer = null
      petState = activePetSessions.size ? 'waiting' : 'idle'
      sendPetState()
    }, resetAfter)
    petStateResetTimer.unref?.()
  }
}

function observeRuntimeEvent({ event, sessionId } = {}) {
  const id = String(sessionId || '')
  if (event === 'done' || event === 'error') {
    if (id) activePetSessions.delete(id)
    publishPetState(event === 'error' ? 'failed' : 'waving', { resetAfter: event === 'error' ? 2200 : 1400 })
    return
  }
  const state = petStateForAgentEvent(event)
  if (!state) return
  if (id) activePetSessions.add(id)
  publishPetState(state)
}

function persistPetPosition() {
  if (!petWindow || petWindow.isDestroyed()) return
  const [petX, petY] = petWindow.getPosition()
  saveDesktopPreferences({ petX, petY })
}

function destroyDesktopPet() {
  if (petStateResetTimer) clearTimeout(petStateResetTimer)
  petStateResetTimer = null
  petDragStart = null
  if (!petWindow || petWindow.isDestroyed()) {
    petWindow = null
    return
  }
  petClosing = true
  petWindow.destroy()
  petWindow = null
  petClosing = false
}

async function createDesktopPet({ notifyOnError = false } = {}) {
  if (petWindow && !petWindow.isDestroyed()) {
    petWindow.showInactive()
    return petWindow
  }
  if (petWindowCreation) return petWindowCreation
  petWindowCreation = (async () => {
    try {
      const preferences = loadDesktopPreferences()
      if (!preferences.petEnabled) return null
      const installedPet = resolveInstalledDesktopPet(preferences.petSlug)
      const position = resolvePetPosition(
        { x: preferences.petX, y: preferences.petY },
        screen.getAllDisplays(),
        screen.getPrimaryDisplay(),
      )
      const appRoot = app.getAppPath()
      petWindow = new BrowserWindow({
        ...position,
        width: PET_WINDOW_WIDTH,
        height: PET_WINDOW_HEIGHT,
        show: false,
        frame: false,
        transparent: true,
        resizable: false,
        maximizable: false,
        minimizable: false,
        fullscreenable: false,
        skipTaskbar: true,
        alwaysOnTop: true,
        hasShadow: false,
        backgroundColor: '#00000000',
        webPreferences: {
          preload: join(appRoot, 'electron', 'pet-preload.cjs'),
          contextIsolation: true,
          nodeIntegration: false,
          sandbox: true,
        },
      })
      petWindow.setAlwaysOnTop(true, 'floating')
      petWindow.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true })
      petWindow.on('move', persistPetPosition)
      petWindow.on('close', (event) => {
        if (quitting || petClosing) return
        event.preventDefault()
        petWindow?.hide()
      })
      petWindow.on('closed', () => {
        petWindow = null
      })
      petWindow.webContents.setWindowOpenHandler(() => ({ action: 'deny' }))
      petWindow.webContents.on('will-navigate', (event) => event.preventDefault())
      petWindow.webContents.once('did-finish-load', () => {
        if (!petWindow || petWindow.isDestroyed()) return
        const spriteDataUrl = `data:${installedPet.mime};base64,${installedPet.buffer.toString('base64')}`
        petWindow.webContents.send('vesper:pet-config', {
          spriteDataUrl,
          sheetWidth: installedPet.width,
          sheetHeight: installedPet.height,
          petName: installedPet.name,
          ...petStatePayload(),
        })
        petWindow.showInactive()
      })
      await petWindow.loadFile(join(appRoot, 'electron', 'pet-window.html'))
      return petWindow
    } catch (error) {
      destroyDesktopPet()
      updateLogger.error('Desktop pet failed to start.', error)
      if (notifyOnError) {
        await dialog.showMessageBox(mainWindow || undefined, {
          type: 'error',
          title: t('pet.errorTitle'),
          message: t('pet.errorMessage'),
          detail: error instanceof Error ? error.message : String(error),
        })
      }
      return null
    }
  })()
  try {
    return await petWindowCreation
  } finally {
    petWindowCreation = null
  }
}

async function setDesktopPetEnabled(enabled, { notifyOnError = true } = {}) {
  saveDesktopPreferences({ petEnabled: enabled })
  updateTrayMenu()
  if (!enabled) {
    destroyDesktopPet()
    return false
  }
  const window = await createDesktopPet({ notifyOnError })
  if (!window) {
    if (notifyOnError) saveDesktopPreferences({ petEnabled: false })
    updateTrayMenu()
    return false
  }
  return true
}

function desktopPreferencesPath() {
  return join(app.getPath('userData'), 'desktop-preferences.json')
}

function normalizeCloseAction(value) {
  if (value === CLOSE_ACTION_TRAY || value === CLOSE_ACTION_QUIT || value === CLOSE_ACTION_ASK) return value
  return CLOSE_ACTION_ASK
}

function loadDesktopPreferences() {
  try {
    const raw = readFileSync(desktopPreferencesPath(), 'utf8')
    const data = JSON.parse(raw)
    return {
      closeAction: normalizeCloseAction(data?.closeAction),
      language: isSupportedLanguage(data?.language) ? data.language : null,
      petEnabled: data?.petEnabled === true,
      petSlug: typeof data?.petSlug === 'string' ? data.petSlug : '',
      petX: Number.isFinite(data?.petX) ? Math.round(data.petX) : null,
      petY: Number.isFinite(data?.petY) ? Math.round(data.petY) : null,
    }
  } catch {
    return { closeAction: CLOSE_ACTION_ASK, language: null, petEnabled: false, petSlug: '', petX: null, petY: null }
  }
}

function saveDesktopPreferences(patch = {}) {
  const current = loadDesktopPreferences()
  const next = {
    ...current,
    ...patch,
    closeAction: normalizeCloseAction(patch.closeAction ?? current.closeAction),
    language: isSupportedLanguage(patch.language)
      ? patch.language
      : (isSupportedLanguage(current.language) ? current.language : null),
    petEnabled: patch.petEnabled === undefined ? current.petEnabled : patch.petEnabled === true,
    petSlug: typeof patch.petSlug === 'string' ? patch.petSlug : current.petSlug,
    petX: Number.isFinite(patch.petX) ? Math.round(patch.petX) : current.petX,
    petY: Number.isFinite(patch.petY) ? Math.round(patch.petY) : current.petY,
  }
  writeFileSync(desktopPreferencesPath(), `${JSON.stringify(next, null, 2)}\n`, 'utf8')
  return next
}

function applyDesktopLanguage(language, { persist = false } = {}) {
  if (!isSupportedLanguage(language)) return getDesktopLanguage()
  const next = setDesktopLanguage(language)
  if (persist) saveDesktopPreferences({ language: next })
  updateTrayMenu()
  sendPetState()
  return next
}

function restoreDesktopLanguagePreference() {
  const { language } = loadDesktopPreferences()
  if (language) setDesktopLanguage(language)
}

function closeActionLabel(action) {
  if (action === CLOSE_ACTION_TRAY) return t('tray.closeActionTray')
  if (action === CLOSE_ACTION_QUIT) return t('tray.closeActionQuit')
  return t('tray.closeActionAsk')
}

function buildTrayMenuTemplate() {
  const { closeAction, petEnabled } = loadDesktopPreferences()
  return [
    {
      label: t('tray.showMainWindow'),
      click: () => { void showMainWindow() },
    },
    {
      label: t('tray.desktopPet'),
      type: 'checkbox',
      checked: petEnabled,
      click: (item) => { void setDesktopPetEnabled(item.checked) },
    },
    {
      label: t('tray.desktopPetCredit'),
      enabled: petEnabled,
      click: () => { void openExternalUrl(PETDEX_PAGE_URL) },
    },
    { type: 'separator' },
    {
      label: t('tray.closeAction', { action: closeActionLabel(closeAction) }),
      enabled: false,
    },
    ...(closeAction === CLOSE_ACTION_ASK
      ? []
      : [{
          label: t('tray.askOnCloseAgain'),
          click: () => {
            saveDesktopPreferences({ closeAction: CLOSE_ACTION_ASK })
            updateTrayMenu()
          },
        }]),
    { type: 'separator' },
    {
      label: t('tray.quit'),
      click: () => {
        app.quit()
      },
    },
  ]
}

function updateTrayMenu() {
  if (!tray) return
  tray.setContextMenu(Menu.buildFromTemplate(buildTrayMenuTemplate()))
}

async function handleWindowCloseRequest() {
  if (quitting || !mainWindow || mainWindow.isDestroyed()) return
  if (!tray) {
    app.quit()
    return
  }

  const { closeAction } = loadDesktopPreferences()
  if (closeAction === CLOSE_ACTION_TRAY) {
    hideMainWindowToTray()
    return
  }
  if (closeAction === CLOSE_ACTION_QUIT) {
    app.quit()
    return
  }
  if (closePromptOpen) return

  closePromptOpen = true
  try {
    const result = await dialog.showMessageBox(mainWindow, {
      type: 'question',
      buttons: [t('closeDialog.minimizeToTray'), t('closeDialog.quit'), t('closeDialog.cancel')],
      defaultId: 0,
      cancelId: 2,
      title: t('closeDialog.title'),
      message: t('closeDialog.message'),
      detail: t('closeDialog.detail'),
      checkboxLabel: t('closeDialog.rememberChoice'),
      checkboxChecked: false,
      noLink: true,
    })

    if (result.response === 2 || quitting) return

    const action = result.response === 0 ? CLOSE_ACTION_TRAY : CLOSE_ACTION_QUIT
    if (result.checkboxChecked) {
      saveDesktopPreferences({ closeAction: action })
      updateTrayMenu()
    }

    if (action === CLOSE_ACTION_TRAY) hideMainWindowToTray()
    else app.quit()
  } finally {
    closePromptOpen = false
  }
}

function createTray() {
  if (tray) return tray
  const iconPath = resolveTrayIconPath()
  if (!iconPath) {
    updateLogger.warn('Tray icon not found; system tray was not created.')
    return null
  }
  const image = nativeImage.createFromPath(iconPath)
  if (image.isEmpty()) {
    updateLogger.warn('Tray icon could not be loaded; system tray was not created.', { iconPath })
    return null
  }
  if (process.platform === 'darwin') image.setTemplateImage(true)
  tray = new Tray(process.platform === 'win32' ? image.resize({ width: 16, height: 16 }) : image)
  tray.setToolTip('Vesper')
  updateTrayMenu()
  tray.on('click', () => { void showMainWindow() })
  tray.on('double-click', () => { void showMainWindow() })
  return tray
}

function titleBarOptions() {
  if (process.platform === 'darwin') return { titleBarStyle: 'hiddenInset' }
  return {
    titleBarStyle: 'hidden',
    titleBarOverlay: {
      color: '#00000000',
      symbolColor: nativeTheme.shouldUseDarkColors ? '#f4f4f5' : '#18181b',
      height: 42,
    },
  }
}

function updateTitleBarOverlay() {
  if (!mainWindow || process.platform === 'darwin' || typeof mainWindow.setTitleBarOverlay !== 'function') return
  mainWindow.setTitleBarOverlay({
    color: '#00000000',
    symbolColor: nativeTheme.shouldUseDarkColors ? '#f4f4f5' : '#18181b',
    height: 42,
  })
}

async function openExternalUrl(value) {
  try {
    const url = new URL(value)
    if (!['http:', 'https:', 'mailto:'].includes(url.protocol)) return false
    await shell.openExternal(url.href)
    return true
  } catch {
    return false
  }
}

async function createWindow() {
  const appRoot = app.getAppPath()
  const icon = resolveAppIconPath(appRoot)
  if (!vesperServer) {
    vesperServer = await createVesperServer({
      root: appRoot,
      runtimeCwd: process.env.VESPER_WORKSPACE_DIR || homedir(),
      dataDir: process.env.VESPER_AGENT_DIR || join(homedir(), '.vesper', 'agent'),
      production: true,
      port: 0,
      host: '127.0.0.1',
      browserAutomationDriver: createElectronBrowserAutomationDriver(),
      runtimeEventObserver: observeRuntimeEvent,
    })
  }
  mainWindow = new BrowserWindow({
    width: 1440,
    height: 920,
    minWidth: 980,
    minHeight: 680,
    show: false,
    backgroundColor: nativeTheme.shouldUseDarkColors ? '#111113' : '#ffffff',
    autoHideMenuBar: true,
    ...(icon ? { icon } : {}),
    ...titleBarOptions(),
    webPreferences: {
      preload: join(appRoot, 'electron', 'preload.cjs'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  })
  Menu.setApplicationMenu(null)
  mainWindow.webContents.setWindowOpenHandler(({ url }) => {
    void openExternalUrl(url)
    return { action: 'deny' }
  })
  mainWindow.webContents.on('will-navigate', (event, url) => {
    try {
      if (new URL(url).origin === vesperServer.url) return
    } catch {
      // Invalid and non-web URLs are always kept outside the renderer.
    }
    event.preventDefault()
    void openExternalUrl(url)
  })
  mainWindow.once('ready-to-show', () => mainWindow?.show())
  // Close button asks tray/quit (or uses the remembered preference).
  mainWindow.on('close', (event) => {
    if (quitting || !tray) return
    event.preventDefault()
    void handleWindowCloseRequest()
  })
  mainWindow.on('closed', () => { mainWindow = null })
  await mainWindow.loadURL(vesperServer.url)
}

function isPetSender(event) {
  return Boolean(petWindow && !petWindow.isDestroyed() && event.sender === petWindow.webContents)
}

function showDesktopPetContextMenu() {
  if (!petWindow || petWindow.isDestroyed()) return
  Menu.buildFromTemplate([
    { label: t('tray.showMainWindow'), click: () => { void showMainWindow() } },
    { label: t('tray.desktopPetCredit'), click: () => { void openExternalUrl(PETDEX_PAGE_URL) } },
    { type: 'separator' },
    { label: t('pet.hide'), click: () => { void setDesktopPetEnabled(false) } },
    { label: t('tray.quit'), click: () => app.quit() },
  ]).popup({ window: petWindow })
}

function registerIpc() {
  ipcMain.on('vesper:pet-drag', (event, input = {}) => {
    if (!isPetSender(event)) return
    const screenX = Number(input.screenX)
    const screenY = Number(input.screenY)
    if (!Number.isFinite(screenX) || !Number.isFinite(screenY)) return
    if (input.phase === 'start') {
      const [x, y] = petWindow.getPosition()
      petDragStart = { screenX, screenY, x, y }
      return
    }
    if (input.phase === 'move' && petDragStart) {
      petWindow.setPosition(
        Math.round(petDragStart.x + screenX - petDragStart.screenX),
        Math.round(petDragStart.y + screenY - petDragStart.screenY),
        false,
      )
      return
    }
    if (input.phase === 'end') {
      petDragStart = null
      persistPetPosition()
    }
  })
  ipcMain.on('vesper:pet-interact', (event) => {
    if (!isPetSender(event)) return
    publishPetState('jumping', { resetAfter: 1000 })
  })
  ipcMain.on('vesper:pet-context-menu', (event) => {
    if (isPetSender(event)) showDesktopPetContextMenu()
  })
  ipcMain.on('vesper:pet-show-main-window', (event) => {
    if (isPetSender(event)) void showMainWindow()
  })

  ipcMain.handle('vesper:get-pet-status', () => desktopPetStatus())
  ipcMain.handle('vesper:set-pet-enabled', async (_event, enabled) => {
    await setDesktopPetEnabled(enabled === true)
    return desktopPetStatus()
  })
  ipcMain.handle('vesper:search-pets', async (_event, query) => {
    const needle = String(query || '').trim().toLowerCase()
    const manifest = await petdexManifest()
    return manifest
      .filter((pet) => !needle || pet.slug.includes(needle) || pet.displayName.toLowerCase().includes(needle))
      .slice(0, needle ? 40 : 12)
      .map(({ slug, displayName }) => ({ slug, displayName }))
  })
  ipcMain.handle('vesper:install-pet', (_event, slug) => installManagedDesktopPet(slug))
  ipcMain.handle('vesper:select-pet', (_event, slug) => selectDesktopPet(slug))
  ipcMain.handle('vesper:open-petdex', async () => openExternalUrl(PETDEX_PAGE_URL))

  ipcMain.handle('vesper:get-app-info', () => ({
    desktop: true,
    packaged: app.isPackaged,
    version: app.getVersion(),
    platform: process.platform,
    arch: process.arch,
    language: getDesktopLanguage(),
    releasesUrl: RELEASES_URL,
    update: updateState,
  }))
  ipcMain.handle('vesper:set-language', (_event, language) => applyDesktopLanguage(language, { persist: true }))
  ipcMain.handle('vesper:check-for-updates', () => checkForUpdates())
  ipcMain.handle('vesper:download-update', async () => {
    const canResume = updateState.state === 'error' && updateState.canResume
    const canDownload = updateState.state === 'available' || canResume
    if (!app.isPackaged || !canDownload || !updateState.canDownload) {
      await openExternalUrl(updateState.releaseUrl || RELEASES_URL)
      return publishUpdate({ ...updateState, message: t('update.openedGitHubReleases') })
    }
    publishUpdate({ state: 'downloading', canResume: false, percent: 0, message: '' })
    await autoUpdater.downloadUpdate()
    return updateState
  })
  ipcMain.handle('vesper:install-update', async () => {
    if (updateState.state !== 'downloaded' || installingUpdate) return false
    installingUpdate = true
    quitting = true
    updateLogger.info('Installing downloaded update and requesting application restart.', { version: updateState.availableVersion || '' })
    const result = await prepareApplicationShutdown({ exit: false })
    updateLogger.info('Application resources released; launching update installer.', result)
    autoUpdater.quitAndInstall(false, true)
    return true
  })
  ipcMain.handle('vesper:open-releases', async () => {
    await openExternalUrl(updateState.releaseUrl || RELEASES_URL)
    return true
  })
  ipcMain.handle('vesper:open-update-log', () => {
    if (!updateLogPath || !existsSync(updateLogPath)) return false
    shell.showItemInFolder(updateLogPath)
    return true
  })
  ipcMain.handle('vesper:get-notification-status', () => getDesktopNotificationStatus({
    appUserModelId: APP_USER_MODEL_ID,
    isSupported: ElectronNotification.isSupported(),
  }))
  ipcMain.handle('vesper:open-notification-settings', async () => {
    if (process.platform !== 'win32') return false
    await shell.openExternal(WINDOWS_NOTIFICATION_SETTINGS_URL)
    return true
  })
  ipcMain.handle('vesper:show-notification', async (_event, input = {}) => {
    const status = await getDesktopNotificationStatus({
      appUserModelId: APP_USER_MODEL_ID,
      isSupported: ElectronNotification.isSupported(),
    })
    if (status.permission !== 'granted') return { shown: false, ...status }
    const title = String(input.title || '').trim().slice(0, 120)
    const body = String(input.body || '').trim().slice(0, 2_000)
    if (!title) return { shown: false, ...status, reason: 'invalid-title' }
    const notification = new ElectronNotification({ title, body })
    activeDesktopNotifications.add(notification)
    const cleanup = () => activeDesktopNotifications.delete(notification)
    notification.once('close', cleanup)
    notification.once('failed', cleanup)
    notification.on('click', () => {
      void showMainWindow()
    })
    notification.show()
    const cleanupTimer = setTimeout(cleanup, 30_000)
    cleanupTimer.unref?.()
    return { shown: true, ...status }
  })
}

if (!app.requestSingleInstanceLock()) app.quit()
else {
  app.on('second-instance', () => {
    void showMainWindow()
  })
  app.whenReady().then(async () => {
    app.setAppUserModelId(APP_USER_MODEL_ID)
    restoreDesktopLanguagePreference()
    configureUpdater()
    registerIpc()
    nativeTheme.on('updated', updateTitleBarOverlay)
    createTray()
    await createWindow()
    if (loadDesktopPreferences().petEnabled) await setDesktopPetEnabled(true, { notifyOnError: false })
    setTimeout(() => { void checkForUpdates({ silent: true }) }, 3_000)
    app.on('activate', () => { void showMainWindow() })
  }).catch((error) => {
    updateLogger.error('Vesper failed to start.', error)
    dialog.showErrorBox('Vesper failed to start', `${error instanceof Error ? error.message : String(error)}\n\nUpdate log: ${updateLogPath || 'not initialized'}`)
    app.exit(1)
  })
}

// Keep the process alive while the main window is only hidden in the tray.
app.on('window-all-closed', () => {
  if (quitting || tray) return
  if (process.platform !== 'darwin') app.quit()
})
app.on('before-quit', (event) => {
  if (quitting) return
  event.preventDefault()
  quitting = true
  void prepareApplicationShutdown()
})

process.on('uncaughtException', (error) => {
  updateLogger.error('Uncaught main-process exception.', error)
  if (app.isReady()) dialog.showErrorBox('Vesper encountered an error', `${error.message}\n\nUpdate log: ${updateLogPath || 'not initialized'}`)
  app.exit(1)
})

process.on('unhandledRejection', (error) => {
  updateLogger.error('Unhandled main-process rejection.', error)
})
