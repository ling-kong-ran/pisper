import { useCallback, useEffect, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import type { DesktopPetStatus } from '@/types/update'

type PetState = 'idle' | 'waving' | 'jumping' | 'failed' | 'waiting' | 'running' | 'review'

type PetPosition = { x: number; y: number }

type PointerDrag = {
  id: number
  offsetX: number
  offsetY: number
  startX: number
  startY: number
  moved: boolean
}

const PET_WIDTH = 192
const PET_HEIGHT = 288
const POSITION_KEY = 'pisper-web-desktop-pet-position'
const PET_STATES: Record<PetState, { row: number; frames: number; durationMs: number }> = {
  idle: { row: 0, frames: 6, durationMs: 1100 },
  waving: { row: 3, frames: 4, durationMs: 700 },
  jumping: { row: 4, frames: 5, durationMs: 840 },
  failed: { row: 5, frames: 8, durationMs: 1220 },
  waiting: { row: 6, frames: 6, durationMs: 1010 },
  running: { row: 7, frames: 6, durationMs: 820 },
  review: { row: 8, frames: 6, durationMs: 1030 },
}

function clampPosition(position: PetPosition): PetPosition {
  return {
    x: Math.max(0, Math.min(window.innerWidth - PET_WIDTH, position.x)),
    y: Math.max(0, Math.min(window.innerHeight - PET_HEIGHT, position.y)),
  }
}

function initialPosition(): PetPosition {
  try {
    const saved = JSON.parse(localStorage.getItem(POSITION_KEY) || '{}')
    if (Number.isFinite(saved?.x) && Number.isFinite(saved?.y)) return clampPosition(saved)
  } catch {
    // Invalid local positions fall back to the lower-right corner.
  }
  return clampPosition({
    x: window.innerWidth - PET_WIDTH - 20,
    y: window.innerHeight - PET_HEIGHT - 20,
  })
}

function normalizedState(value: string | undefined): PetState {
  return value && value in PET_STATES ? (value as PetState) : 'idle'
}

export function WebDesktopPet() {
  const { t } = useI18n()
  const [status, setStatus] = useState<DesktopPetStatus | null>(null)
  const [position, setPosition] = useState<PetPosition>(initialPosition)
  const [interactionState, setInteractionState] = useState<PetState | null>(null)
  const pointer = useRef<PointerDrag | null>(null)
  const interactionTimer = useRef<number | undefined>(undefined)

  const refresh = useCallback(async () => {
    try {
      setStatus(await apiJson<DesktopPetStatus>('/api/desktop-pet'))
    } catch {
      // The pet is optional; transient polling failures must not affect the application.
    }
  }, [])

  useEffect(() => {
    if (window.pisperDesktop?.getPetStatus) return undefined
    let timer: number | undefined
    const startPolling = () => {
      if (document.visibilityState !== 'visible' || timer !== undefined) return
      void refresh()
      timer = window.setInterval(refresh, 1200)
    }
    const stopPolling = () => {
      if (timer === undefined) return
      window.clearInterval(timer)
      timer = undefined
    }
    const onVisibilityChange = () => {
      if (document.visibilityState === 'visible') startPolling()
      else stopPolling()
    }
    const onChanged = (event: Event) => {
      const next = (event as CustomEvent<DesktopPetStatus>).detail
      if (next) setStatus(next)
      else void refresh()
    }
    const onResize = () => setPosition((current) => clampPosition(current))
    startPolling()
    document.addEventListener('visibilitychange', onVisibilityChange)
    window.addEventListener('pisper:desktop-pet-changed', onChanged)
    window.addEventListener('resize', onResize)
    return () => {
      stopPolling()
      window.clearTimeout(interactionTimer.current)
      document.removeEventListener('visibilitychange', onVisibilityChange)
      window.removeEventListener('pisper:desktop-pet-changed', onChanged)
      window.removeEventListener('resize', onResize)
    }
  }, [refresh])

  if (window.pisperDesktop?.getPetStatus || !status?.running || !status.spriteUrl) return null

  const stateName = interactionState || normalizedState(status.state)
  const animation = PET_STATES[stateName]
  const bubbles: Partial<Record<PetState, string>> = {
    review: t('config:desktopPetSettings.bubbleThinking'),
    running: t('config:desktopPetSettings.bubbleWorking'),
    waiting: t('config:desktopPetSettings.bubbleResponding'),
    waving: t('config:desktopPetSettings.bubbleDone'),
    failed: t('config:desktopPetSettings.bubbleFailed'),
    jumping: t('config:desktopPetSettings.bubbleHello'),
  }
  const bubble = bubbles[stateName] || ''

  const finishPointer = (event: React.PointerEvent<HTMLDivElement>) => {
    const active = pointer.current
    if (!active || active.id !== event.pointerId) return
    pointer.current = null
    event.currentTarget.releasePointerCapture(event.pointerId)
    if (active.moved) {
      localStorage.setItem(POSITION_KEY, JSON.stringify(position))
      return
    }
    setInteractionState('jumping')
    window.clearTimeout(interactionTimer.current)
    interactionTimer.current = window.setTimeout(() => setInteractionState(null), 900)
  }

  return (
    <div
      className={`web-desktop-pet ${pointer.current ? 'dragging' : ''}`}
      style={{ left: position.x, top: position.y, opacity: status.opacity ?? 1 }}
      onPointerDown={(event) => {
        if (event.button !== 0) return
        const bounds = event.currentTarget.getBoundingClientRect()
        pointer.current = {
          id: event.pointerId,
          offsetX: event.clientX - bounds.left,
          offsetY: event.clientY - bounds.top,
          startX: event.clientX,
          startY: event.clientY,
          moved: false,
        }
        event.currentTarget.setPointerCapture(event.pointerId)
      }}
      onPointerMove={(event) => {
        const active = pointer.current
        if (!active || active.id !== event.pointerId) return
        if (Math.hypot(event.clientX - active.startX, event.clientY - active.startY) > 4)
          active.moved = true
        setPosition(
          clampPosition({
            x: event.clientX - active.offsetX,
            y: event.clientY - active.offsetY,
          }),
        )
      }}
      onPointerUp={finishPointer}
      onPointerCancel={finishPointer}
      aria-label={t('config:desktopPetSettings.webPetLabel', {
        name: status.selectedName || status.selectedSlug,
      })}
    >
      <div className={`web-desktop-pet-bubble ${bubble ? 'visible' : ''}`} role="status">
        {bubble}
      </div>
      <div className="web-desktop-pet-frame" role="img">
        <div
          className="web-desktop-pet-sprite"
          style={
            {
              '--pet-sprite-url': `url("${status.spriteUrl}")`,
              '--pet-sheet-width': `${status.sheetWidth || 1536}px`,
              '--pet-sheet-height': `${status.sheetHeight || 1872}px`,
              '--pet-row': animation.row,
              '--pet-frames': animation.frames,
              '--pet-duration': `${animation.durationMs}ms`,
            } as React.CSSProperties
          }
        />
      </div>
    </div>
  )
}
