// 桌面宠物 Web 壳：在页面角落渲染桌面宠物（SVG 精灵 + 交互动画），
// 通过桌面桥接同步启用/选择状态。
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

  // 刷新桌面宠物状态：宠物为可选功能，轮询失败不影响应用。
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
      className={`web-desktop-pet max-[640px]:[transform:scale(0.75)] max-[640px]:origin-[bottom_right] fixed z-[70] w-[192px] h-[288px] cursor-grab [touch-action:none] select-none [filter:drop-shadow(0_12px_14px_rgb(15_23_42_/_0.16))] ${pointer.current ? 'dragging [.web-desktop-pet&]:cursor-grabbing' : ''}`}
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
      <div
        className={`web-desktop-pet-bubble motion-reduce:[transition:none] absolute z-[2] [left:50%] [bottom:210px] max-w-[180px] [padding:6px_10px] [transform:translateX(-50%)_translateY(4px)] [border:1px_solid_color-mix(in_srgb,_var(--stroke)_75%,_transparent)] rounded-[10px] bg-[color-mix(in_srgb,_var(--surface)_94%,_transparent)] shadow-[0_8px_24px_rgb(15_23_42_/_0.2)] text-[var(--text)] text-[11px] leading-[1.4] opacity-0 pointer-events-none text-center [transition:opacity_0.18s_ease,_transform_0.18s_ease] whitespace-nowrap ${bubble ? 'visible [.web-desktop-pet-bubble&]:opacity-100 [.web-desktop-pet-bubble&]:[transform:translateX(-50%)_translateY(0)]' : ''}`}
        role="status"
      >
        {bubble}
      </div>
      <div
        className="absolute left-0 bottom-0 w-[192px] h-[208px] [contain:layout_paint] overflow-hidden"
        role="img"
      >
        <div
          className="web-desktop-pet-sprite [--pet-sprite-y:calc(var(--pet-row)_*_-208px)] [--pet-sprite-end-x:calc(var(--pet-frames)_*_-192px)] w-[192px] h-[208px] [background-image:var(--pet-sprite-url)] [background-repeat:no-repeat] [background-size:var(--pet-sheet-width,_1536px)_var(--pet-sheet-height,_1872px)] [image-rendering:pixelated] [transform:translateZ(0)] origin-[top_left] [will-change:background-position] [animation:web-pet-state_var(--pet-duration)_steps(var(--pet-frames))_infinite] motion-reduce:[animation:none]"
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
