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
  onUpdateStatus(callback) {
    if (typeof callback !== 'function') return () => {}
    const listener = (_event, status) => callback(status)
    ipcRenderer.on('vesper:update-status', listener)
    return () => ipcRenderer.removeListener('vesper:update-status', listener)
  },
}))
