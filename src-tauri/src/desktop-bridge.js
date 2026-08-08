;(() => {
  if (window.__pisperTauriBridgeInstalled) return
  window.__pisperTauriBridgeInstalled = true

  const invoke = (command, args) => {
    const internals = window.__TAURI_INTERNALS__
    if (!internals?.invoke) return Promise.reject(new Error('Tauri IPC is unavailable.'))
    return internals.invoke(command, args)
  }

  const api = async (path, options = {}) => {
    const response = await fetch(path, {
      ...options,
      headers: {
        ...(options.body ? { 'Content-Type': 'application/json' } : {}),
        ...options.headers,
      },
      body: options.body ? JSON.stringify(options.body) : undefined,
    })
    const value = await response.json().catch(() => ({}))
    if (!response.ok) throw new Error(value.error || `HTTP ${response.status}`)
    return value
  }

  const syncPetWindow = async (status) => {
    await invoke('desktop_pet_sync_menu', { enabled: Boolean(status?.enabled) })
    await invoke('desktop_pet_apply_enabled', { enabled: Boolean(status?.running) })
    return status
  }

  const updateListeners = new Set()
  let updatePoll = 0
  let lastUpdate = ''
  const pollUpdates = async () => {
    try {
      const status = await invoke('desktop_update_status')
      const serialized = JSON.stringify(status)
      if (serialized === lastUpdate) return
      lastUpdate = serialized
      for (const listener of updateListeners) listener(status)
    } catch {}
  }
  const syncUpdatePolling = () => {
    if (updateListeners.size && !updatePoll) {
      updatePoll = window.setInterval(pollUpdates, 500)
      void pollUpdates()
    } else if (!updateListeners.size && updatePoll) {
      window.clearInterval(updatePoll)
      updatePoll = 0
    }
  }

  Object.defineProperty(window, 'pisperDesktop', {
    configurable: false,
    enumerable: true,
    writable: false,
    value: Object.freeze({
      getAppInfo: () => invoke('desktop_get_app_info'),
      pickDirectory: (initialDirectory) => invoke('desktop_pick_directory', { initialDirectory }),
      setLanguage: (language) => invoke('desktop_set_language', { language }),
      getCliStatus: () => invoke('desktop_get_cli_status'),
      installCli: () => invoke('desktop_install_cli'),
      uninstallCli: () => invoke('desktop_uninstall_cli'),
      checkForUpdates: () => invoke('desktop_check_for_updates'),
      downloadUpdate: () => invoke('desktop_download_update'),
      installUpdate: () => invoke('desktop_install_update'),
      componentUpdateStatus: () => invoke('desktop_component_update_status'),
      checkComponentUpdates: () => invoke('desktop_check_component_updates'),
      installComponentUpdates: () => invoke('desktop_install_component_updates'),
      restartForComponentUpdate: () => invoke('desktop_restart_for_component_update'),
      openReleases: () => invoke('desktop_open_releases'),
      openUpdateLog: () => invoke('desktop_open_update_log'),
      getNotificationStatus: () => invoke('desktop_get_notification_status'),
      openNotificationSettings: () => invoke('desktop_open_notification_settings'),
      showNotification: (notification) =>
        invoke('desktop_show_notification', { input: notification }),
      getPetStatus: () => api('/api/desktop-pet'),
      setPetEnabled: async (enabled) =>
        syncPetWindow(await api('/api/desktop-pet/enabled', { method: 'POST', body: { enabled } })),
      setPetOpacity: (opacity) =>
        api('/api/desktop-pet/opacity', { method: 'POST', body: { opacity } }),
      searchPets: (query) => api(`/api/desktop-pet/catalog?query=${encodeURIComponent(query)}`),
      installPet: async (slug) =>
        syncPetWindow(await api('/api/desktop-pet/install', { method: 'POST', body: { slug } })),
      selectPet: async (slug) =>
        syncPetWindow(await api('/api/desktop-pet/select', { method: 'POST', body: { slug } })),
      openPetdex: () => invoke('desktop_open_url', { url: 'https://petdex.dev' }),
      onUpdateStatus(callback) {
        if (typeof callback !== 'function') return () => {}
        updateListeners.add(callback)
        syncUpdatePolling()
        return () => {
          updateListeners.delete(callback)
          syncUpdatePolling()
        }
      },
    }),
  })
})()
