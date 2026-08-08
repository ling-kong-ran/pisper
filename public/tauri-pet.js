(() => {
  const STATES = {
    idle: { row: 0, frames: 6, durationMs: 1100 },
    waving: { row: 3, frames: 4, durationMs: 700 },
    jumping: { row: 4, frames: 5, durationMs: 840 },
    failed: { row: 5, frames: 8, durationMs: 1220 },
    waiting: { row: 6, frames: 6, durationMs: 1010 },
    running: { row: 7, frames: 6, durationMs: 820 },
    review: { row: 8, frames: 6, durationMs: 1030 },
  }
  const BUBBLES = {
    'zh-CN': {
      review: '正在思考...',
      running: '正在工作...',
      waiting: '正在组织回复...',
      waving: '完成了',
      failed: '需要看一下',
      jumping: '你好',
    },
    'en-US': {
      review: 'Thinking...',
      running: 'Working...',
      waiting: 'Preparing a reply...',
      waving: 'Done',
      failed: 'Needs attention',
      jumping: 'Hello',
    },
  }
  const pet = document.querySelector('#pet')
  const sprite = document.querySelector('#sprite')
  const bubble = document.querySelector('#bubble')
  let status = null
  let interaction = ''
  let interactionTimer = 0

  const invoke = (command, args) => window.__TAURI_INTERNALS__?.invoke(command, args)
  const language = () => localStorage.getItem('pisper-language') || 'zh-CN'
  const stateName = () => (interaction && STATES[interaction] ? interaction : STATES[status?.state] ? status.state : 'idle')

  function render() {
    const running = Boolean(status?.running && status?.spriteUrl)
    void invoke('desktop_pet_set_visible', { visible: running })
    if (!running) {
      pet.classList.remove('visible')
      return
    }
    const name = stateName()
    const animation = STATES[name]
    const message = BUBBLES[language()]?.[name] || BUBBLES['zh-CN'][name] || ''
    pet.style.setProperty('--pet-opacity', String(status.opacity ?? 1))
    sprite.style.setProperty('--pet-sprite-url', `url("${status.spriteUrl}")`)
    sprite.style.setProperty('--pet-sheet-width', `${status.sheetWidth || 1536}px`)
    sprite.style.setProperty('--pet-sheet-height', `${status.sheetHeight || 1872}px`)
    sprite.style.setProperty('--pet-row', String(animation.row))
    sprite.style.setProperty('--pet-frames', String(animation.frames))
    sprite.style.setProperty('--pet-duration', `${animation.durationMs}ms`)
    bubble.textContent = message
    bubble.classList.toggle('visible', Boolean(message))
    pet.setAttribute('aria-label', status.selectedName || status.selectedSlug || 'Pisper Pet')
    pet.classList.add('visible')
  }

  async function setEnabled(enabled) {
    try {
      const response = await fetch('/api/desktop-pet/enabled', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ enabled }),
      })
      if (!response.ok) return
      status = await response.json()
      await invoke('desktop_pet_sync_menu', { enabled: Boolean(status?.enabled) })
      await invoke('desktop_pet_apply_enabled', { enabled: Boolean(status?.running) })
      render()
    } catch {
      // A failed preference update leaves the last durable desktop-pet state in place.
    }
  }

  async function refresh() {
    try {
      const response = await fetch('/api/desktop-pet', { cache: 'no-store' })
      if (!response.ok) return
      status = await response.json()
      await invoke('desktop_pet_sync_menu', { enabled: Boolean(status?.enabled) })
      render()
    } catch {
      // The sidecar owns retry and lifecycle; a transient poll failure only hides stale state.
    }
  }

  window.__PISPER_DESKTOP_PET_SET_ENABLED = (enabled) => void setEnabled(Boolean(enabled))

  document.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return
    document.body.classList.add('dragging')
    void invoke('desktop_pet_start_dragging')
  })
  document.addEventListener('pointerup', () => document.body.classList.remove('dragging'))
  document.addEventListener('dblclick', () => void invoke('desktop_show_main_window'))
  document.addEventListener('contextmenu', (event) => {
    event.preventDefault()
    void invoke('desktop_pet_show_context_menu')
  })
  document.addEventListener('click', () => {
    interaction = 'jumping'
    window.clearTimeout(interactionTimer)
    interactionTimer = window.setTimeout(() => {
      interaction = ''
      render()
    }, 900)
    render()
  })

  void refresh()
  window.setInterval(refresh, 1200)
})()
