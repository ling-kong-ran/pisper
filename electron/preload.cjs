const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('pisperDesktop', Object.freeze({
  platform: process.platform,
  getAppInfo: () => ipcRenderer.invoke('pisper:get-app-info'),
  setLanguage: (language) => ipcRenderer.invoke('pisper:set-language', language),
  checkForUpdates: () => ipcRenderer.invoke('pisper:check-for-updates'),
  downloadUpdate: () => ipcRenderer.invoke('pisper:download-update'),
  installUpdate: () => ipcRenderer.invoke('pisper:install-update'),
  openReleases: () => ipcRenderer.invoke('pisper:open-releases'),
  openUpdateLog: () => ipcRenderer.invoke('pisper:open-update-log'),
  getNotificationStatus: () => ipcRenderer.invoke('pisper:get-notification-status'),
  openNotificationSettings: () => ipcRenderer.invoke('pisper:open-notification-settings'),
  showNotification: (notification) => ipcRenderer.invoke('pisper:show-notification', notification),
  getPetStatus: () => ipcRenderer.invoke('pisper:get-pet-status'),
  setPetEnabled: (enabled) => ipcRenderer.invoke('pisper:set-pet-enabled', enabled),
  setPetOpacity: (opacity) => ipcRenderer.invoke('pisper:set-pet-opacity', opacity),
  searchPets: (query) => ipcRenderer.invoke('pisper:search-pets', query),
  installPet: (slug) => ipcRenderer.invoke('pisper:install-pet', slug),
  selectPet: (slug) => ipcRenderer.invoke('pisper:select-pet', slug),
  openPetdex: () => ipcRenderer.invoke('pisper:open-petdex'),
  onUpdateStatus(callback) {
    if (typeof callback !== 'function') return () => {}
    const listener = (_event, status) => callback(status)
    ipcRenderer.on('pisper:update-status', listener)
    return () => ipcRenderer.removeListener('pisper:update-status', listener)
  },
}))
