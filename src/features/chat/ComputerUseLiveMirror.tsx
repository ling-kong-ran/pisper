// Computer Use 实时窗口镜像：agent 操作目标窗口时，经桌面壳桥接持续拉取帧流，
// 在聊天活动区就地渲染“实时窗口”。捕获发生在原生层（按窗口、不抢焦点），
// 用户前台不被打扰；桥接不可用（Web/移动/无权限）时回退到工具结果静态截图。
import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { Monitor, ShieldAlert } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { DesktopBridge, DesktopComputerUseFrameEvent } from '@/types/update.ts'
import type { EntityRecord } from '@/types/chat'

function bridge(): DesktopBridge | null {
  const desktop = (globalThis as { pisperDesktop?: DesktopBridge }).pisperDesktop
  return desktop?.computerUseStartStream ? desktop : null
}

type MirrorFrame = {
  url: string
  width: number
  height: number
  timestampMs: number
}

type MirrorStatus = 'connecting' | 'live' | 'unavailable'

type ComputerUseLiveMirrorProps = {
  target: EntityRecord
  fallbackImage?: EntityRecord | null
  streaming: boolean
}

// 帧流生命周期：仅在 streaming 且拿到目标窗口时启动；组件卸载或目标切换即停流，
// 避免 agent 转去做别的任务后镜像继续占资源。
function ComputerUseLiveMirrorBase({
  target,
  fallbackImage,
  streaming,
}: ComputerUseLiveMirrorProps) {
  const { t } = useI18n()
  const windowId = Number(target.target?.windowId) || 0
  const [frame, setFrame] = useState<MirrorFrame | null>(null)
  const [status, setStatus] = useState<MirrorStatus>('connecting')
  const [secureInput, setSecureInput] = useState(false)
  const frameUrlRef = useRef<string | null>(null)

  // 系统安全输入状态轮询：Secure Event Input 锁（macOS）/安全桌面（Windows）
  // 激活时合成键盘事件被系统拦截，agent 输入会静默失效——就地提示用户
  // 原因，而不是让用户归因为「自动化失灵」。仅在流式期间轮询；桥接
  // 不可用（Web/移动）时不显示。
  useEffect(() => {
    if (!streaming) return undefined
    const desktop = bridge()
    if (!desktop?.computerUseSecureInputState) return undefined
    let active = true
    const poll = () => {
      desktop
        .computerUseSecureInputState?.()
        .then((state) => {
          if (active) setSecureInput(Boolean(state?.active))
        })
        .catch(() => {
          if (active) setSecureInput(false)
        })
    }
    poll()
    const timer = window.setInterval(poll, 2000)
    return () => {
      active = false
      window.clearInterval(timer)
    }
  }, [streaming])

  useEffect(() => {
    if (!streaming || !windowId) return undefined
    const desktop = bridge()
    if (!desktop) {
      setStatus('unavailable')
      return undefined
    }
    setStatus('connecting')
    let active = true
    desktop
      .computerUseStartStream?.(windowId, (event: DesktopComputerUseFrameEvent) => {
        if (!active) return
        if (event.type === 'frame') {
          frameUrlRef.current = `data:image/jpeg;base64,${event.jpegBase64}`
          setFrame({
            url: frameUrlRef.current,
            width: event.width,
            height: event.height,
            timestampMs: event.timestampMs,
          })
          setStatus('live')
        } else if (event.type === 'error') {
          // 权限缺失/窗口关闭：停流回退静态预览。
          setStatus('unavailable')
        } else if (event.type === 'downgraded') {
          // SCStream 不可用已回退轮询：继续等待帧（降速但不断流）。
          setStatus((prev) => (prev === 'live' ? prev : 'connecting'))
        } else if (event.type === 'stopped') {
          setStatus('unavailable')
        }
      })
      .catch(() => {
        if (active) setStatus('unavailable')
      })
    return () => {
      active = false
      desktop.computerUseStopStream?.(windowId).catch(() => {})
    }
  }, [streaming, windowId])

  const source = frame?.url || (fallbackImage?.url ? String(fallbackImage.url) : '')
  const windowTitle = String(target.target?.windowTitle || target.target?.app || '')
  const live = status === 'live'
  const aspectRatio = useMemo(() => {
    const width = frame?.width || 16
    const height = frame?.height || 10
    return `${width} / ${height}`
  }, [frame?.width, frame?.height])

  return (
    <div
      className="computer-use-live-mirror w-full overflow-hidden rounded-[var(--r-md)] [border:1px_solid_var(--stroke-soft)] bg-[var(--surface-subtle)] [margin:2px_0]"
      data-pisper-live-mirror={live ? 'live' : status}
    >
      {source ? (
        <img
          alt={windowTitle || t('chat:computerUseLive.previewAlt')}
          className="computer-use-live-frame block max-h-[240px] w-full object-contain object-top"
          src={source}
        />
      ) : (
        <div
          className="computer-use-live-placeholder grid min-h-[84px] w-full place-items-center text-[12px] text-[var(--text-muted)]"
          style={{ aspectRatio }}
        >
          {t('chat:computerUseLive.waitingForFirstFrame')}
        </div>
      )}
      {secureInput ? (
        <div
          className="flex items-center gap-[6px] bg-[var(--warning-soft)] px-[8px] py-[4px] text-[11px] text-[var(--warning-strong)]"
          data-pisper-secure-input="active"
        >
          <ShieldAlert size={12} className="flex-none" />
          <span className="min-w-0">{t('chat:computerUseLive.secureInput')}</span>
        </div>
      ) : null}
      <div className="flex items-center justify-between gap-[8px] px-[8px] py-[4px] text-[11px] text-[var(--text-muted)]">
        <span className="flex min-w-0 items-center gap-[5px]">
          <Monitor size={12} />
          <span className="truncate" title={windowTitle}>
            {windowTitle || t('chat:computerUseLive.appWindow')}
          </span>
        </span>
        <span
          className={live ? 'text-[var(--brand-blue-strong)]' : 'text-[var(--text-tertiary)]'}
          data-pisper-live-mirror-status={status}
        >
          {live
            ? t('chat:computerUseLive.live')
            : status === 'connecting'
              ? t('chat:computerUseLive.connecting')
              : t('chat:computerUseLive.staticPreview')}
        </span>
      </div>
    </div>
  )
}

export const ComputerUseLiveMirror = memo(ComputerUseLiveMirrorBase)
