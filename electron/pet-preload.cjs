const { contextBridge, ipcRenderer } = require('electron')

function subscribe(channel, listener) {
  const handler = (_event, value) => listener(value)
  ipcRenderer.on(channel, handler)
  return () => ipcRenderer.removeListener(channel, handler)
}

contextBridge.exposeInMainWorld('vesperPet', {
  onConfig: (listener) => subscribe('vesper:pet-config', listener),
  onState: (listener) => subscribe('vesper:pet-state', listener),
  drag: (input) => ipcRenderer.send('vesper:pet-drag', input),
  interact: () => ipcRenderer.send('vesper:pet-interact'),
  showContextMenu: () => ipcRenderer.send('vesper:pet-context-menu'),
  showMainWindow: () => ipcRenderer.send('vesper:pet-show-main-window'),
})
