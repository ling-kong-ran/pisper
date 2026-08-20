;(() => {
  if (window.__pisperTauriBridgeInstalled) return
  window.__pisperTauriBridgeInstalled = true

  const invoke = (command, args) => {
    const internals = window.__TAURI_INTERNALS__
    if (!internals?.invoke) return Promise.reject(new Error('Tauri IPC is unavailable.'))
    return internals.invoke(command, args)
  }

  class Channel {
    constructor(onmessage) {
      this.onmessage = onmessage || (() => {})
      this.index = 0
      this.pending = []
      this.endIndex = undefined
      this.id = window.__TAURI_INTERNALS__.transformCallback((event) => {
        const index = event.index
        if ('end' in event) {
          if (index === this.index) this.cleanup()
          else this.endIndex = index
          return
        }
        if (index === this.index) {
          this.deliver(event.message)
          while (this.index in this.pending) {
            const message = this.pending[this.index]
            delete this.pending[this.index]
            this.deliver(message)
          }
          if (this.index === this.endIndex) this.cleanup()
        } else {
          this.pending[index] = event.message
        }
      })
    }

    deliver(message) {
      this.onmessage(message)
      this.index += 1
    }

    cleanup() {
      window.__TAURI_INTERNALS__.unregisterCallback(this.id)
    }

    toJSON() {
      return `__CHANNEL__:${this.id}`
    }
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

  Object.defineProperty(window, 'pisperDesktop', {
    configurable: false,
    enumerable: true,
    writable: false,
    value: Object.freeze({
      getAppInfo: () => invoke('desktop_get_app_info'),
      pickDirectory: (initialDirectory) => invoke('desktop_pick_directory', { initialDirectory }),
      pickFiles: (initialDirectory) => invoke('desktop_pick_files', { initialDirectory }),
      setLanguage: (language) => invoke('desktop_set_language', { language }),
      getCliStatus: () => invoke('desktop_get_cli_status'),
      installCli: () => invoke('desktop_install_cli'),
      uninstallCli: () => invoke('desktop_uninstall_cli'),
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
      terminalProfiles: () => invoke('desktop_terminal_profiles'),
      terminalCreate: (options, onEvent) => {
        const channel = new Channel(onEvent)
        return invoke('desktop_terminal_create', { input: options, onEvent: channel })
      },
      terminalWrite: (terminalId, data) =>
        invoke('desktop_terminal_write', { terminalId, data: Array.from(data) }),
      terminalResize: (terminalId, cols, rows) =>
        invoke('desktop_terminal_resize', { terminalId, cols, rows }),
      terminalClose: (terminalId) => invoke('desktop_terminal_close', { terminalId }),
      terminalCloseAll: () => invoke('desktop_terminal_close_all'),
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
    }),
  })
})()
