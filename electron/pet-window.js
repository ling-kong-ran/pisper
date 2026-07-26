const PET_STATES = {
  idle: { row: 0, frames: 6, durationMs: 1100 },
  'running-right': { row: 1, frames: 8, durationMs: 1060 },
  'running-left': { row: 2, frames: 8, durationMs: 1060 },
  waving: { row: 3, frames: 4, durationMs: 700 },
  jumping: { row: 4, frames: 5, durationMs: 840 },
  failed: { row: 5, frames: 8, durationMs: 1220 },
  waiting: { row: 6, frames: 6, durationMs: 1010 },
  running: { row: 7, frames: 6, durationMs: 820 },
  review: { row: 8, frames: 6, durationMs: 1030 },
}

const stage = document.getElementById('stage')
const frame = document.getElementById('frame')
const sprite = document.getElementById('sprite')
const bubble = document.getElementById('bubble')
let pointer = null

function applyState(input = {}) {
  const state = PET_STATES[input.state] || PET_STATES.idle
  sprite.style.setProperty('--sprite-row', state.row)
  sprite.style.setProperty('--sprite-frames', state.frames)
  sprite.style.setProperty('--sprite-duration', `${state.durationMs}ms`)
  bubble.textContent = String(input.bubble || '')
  bubble.classList.toggle('visible', Boolean(input.bubble))
}

window.vesperPet.onConfig((config) => {
  if (config?.spriteDataUrl) sprite.style.setProperty('--sprite-url', `url("${config.spriteDataUrl}")`)
  if (Number.isFinite(config?.sheetWidth)) sprite.style.setProperty('--sheet-width', `${config.sheetWidth}px`)
  if (Number.isFinite(config?.sheetHeight)) sprite.style.setProperty('--sheet-height', `${config.sheetHeight}px`)
  if (config?.petName) frame.setAttribute('aria-label', `${config.petName}, via Petdex`)
  applyState(config)
})
window.vesperPet.onState(applyState)

stage.addEventListener('pointerdown', (event) => {
  if (event.button !== 0) return
  pointer = { id: event.pointerId, startX: event.screenX, startY: event.screenY, moved: false }
  stage.setPointerCapture(event.pointerId)
  stage.classList.add('dragging')
  window.vesperPet.drag({ phase: 'start', screenX: event.screenX, screenY: event.screenY })
})

stage.addEventListener('pointermove', (event) => {
  if (!pointer || pointer.id !== event.pointerId) return
  if (Math.hypot(event.screenX - pointer.startX, event.screenY - pointer.startY) > 4) pointer.moved = true
  window.vesperPet.drag({ phase: 'move', screenX: event.screenX, screenY: event.screenY })
})

function finishPointer(event) {
  if (!pointer || pointer.id !== event.pointerId) return
  const moved = pointer.moved
  pointer = null
  stage.classList.remove('dragging')
  window.vesperPet.drag({ phase: 'end', screenX: event.screenX, screenY: event.screenY })
  if (!moved) window.vesperPet.interact()
}

stage.addEventListener('pointerup', finishPointer)
stage.addEventListener('pointercancel', finishPointer)
stage.addEventListener('dblclick', () => window.vesperPet.showMainWindow())
stage.addEventListener('contextmenu', (event) => {
  event.preventDefault()
  window.vesperPet.showContextMenu()
})
