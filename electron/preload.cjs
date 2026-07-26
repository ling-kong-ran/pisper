const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('vesperDesktop', Object.freeze({
  platform: process.platform,
  getAppInfo: () => ipcRenderer.invoke('vesper:get-app-info'),
  setLanguage: (language) => ipcRenderer.invoke('vesper:set-language', language),
  checkForUpdates: () => ipcRenderer.invoke('vesper:check-for-updates'),
  downloadUpdate: () => ipcRenderer.invoke('vesper:download-update'),
  installUpdate: () => ipcRenderer.invoke('vesper:install-update'),
  openReleases: () => ipcRenderer.invoke('vesper:open-releases'),
  openUpdateLog: () => ipcRenderer.invoke('vesper:open-update-log'),
  getNotificationStatus: () => ipcRenderer.invoke('vesper:get-notification-status'),
  openNotificationSettings: () => ipcRenderer.invoke('vesper:open-notification-settings'),
  showNotification: (notification) => ipcRenderer.invoke('vesper:show-notification', notification),
  getPetStatus: () => ipcRenderer.invoke('vesper:get-pet-status'),
  setPetEnabled: (enabled) => ipcRenderer.invoke('vesper:set-pet-enabled', enabled),
  setPetOpacity: (opacity) => ipcRenderer.invoke('vesper:set-pet-opacity', opacity),
  searchPets: (query) => ipcRenderer.invoke('vesper:search-pets', query),
  installPet: (slug) => ipcRenderer.invoke('vesper:install-pet', slug),
  selectPet: (slug) => ipcRenderer.invoke('vesper:select-pet', slug),
  openPetdex: () => ipcRenderer.invoke('vesper:open-petdex'),
  onUpdateStatus(callback) {
    if (typeof callback !== 'function') return () => {}
    const listener = (_event, status) => callback(status)
    ipcRenderer.on('vesper:update-status', listener)
    return () => ipcRenderer.removeListener('vesper:update-status', listener)
  },
}))
