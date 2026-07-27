const { contextBridge, ipcRenderer } = require('electron')

function subscribe(channel, listener) {
  const handler = (_event, value) => listener(value)
  ipcRenderer.on(channel, handler)
  return () => ipcRenderer.removeListener(channel, handler)
}

contextBridge.exposeInMainWorld('pisperPet', {
  onConfig: (listener) => subscribe('pisper:pet-config', listener),
  onState: (listener) => subscribe('pisper:pet-state', listener),
  drag: (input) => ipcRenderer.send('pisper:pet-drag', input),
  interact: () => ipcRenderer.send('pisper:pet-interact'),
  showContextMenu: () => ipcRenderer.send('pisper:pet-context-menu'),
  showMainWindow: () => ipcRenderer.send('pisper:pet-show-main-window'),
})
