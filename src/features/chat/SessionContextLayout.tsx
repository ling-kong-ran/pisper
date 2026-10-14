// 聊天布局持有右栏宽度，切换会话或抽屉形态时不重挂载主会话。
import { useLayoutEffect, useRef, type ReactNode } from 'react'
import { usePanelRef } from 'react-resizable-panels'
import { useI18n } from '@/app/use-i18n'
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable'
import {
  SESSION_CHAT_MIN_WIDTH,
  SESSION_CONTEXT_DEFAULT_WIDTH,
  SESSION_CONTEXT_MAX_WIDTH,
  SESSION_CONTEXT_MIN_WIDTH,
  type SessionContextPresentation,
} from './session-context-layout'
import { useSessionContextStore } from './session-context-store'

type SessionContextLayoutProps = {
  availableWidth: number
  presentation: SessionContextPresentation
  children: ReactNode
  context: ReactNode
  side?: 'left' | 'right'
}

export function SessionContextLayout({
  availableWidth,
  presentation,
  children,
  context,
  side = 'right',
}: SessionContextLayoutProps) {
  const { t } = useI18n()
  const width = useSessionContextStore((state) => state.width)
  const setWidth = useSessionContextStore((state) => state.setWidth)
  const contextRef = usePanelRef()
  const groupElementRef = useRef<HTMLDivElement>(null)
  const handleElementRef = useRef<HTMLDivElement>(null)
  const aside = presentation === 'aside'

  useLayoutEffect(() => {
    // 窗口变窄只限制当前布局；变宽后恢复偏好，不把临时受限的宽度写回。
    if (aside && availableWidth > 0) contextRef.current?.resize(width)
  }, [aside, availableWidth, contextRef, width, side])

  // 同一组 keyed 节点换序，移动左右栏时保留主会话、输入草稿和滚动容器。
  const conversation = (
    <ResizablePanel
      key="conversation"
      id="chat-conversation"
      minSize={aside ? SESSION_CHAT_MIN_WIDTH : 0}
    >
      {children}
    </ResizablePanel>
  )
  const separator = (
    <ResizableHandle
      key="separator"
      elementRef={handleElementRef}
      aria-label={t('chat:sessionContext.resize')}
      title={t('chat:sessionContext.resizeHint')}
      withHandle
      disableDoubleClick
      onDoubleClick={() => {
        setWidth(SESSION_CONTEXT_DEFAULT_WIDTH)
        contextRef.current?.resize(SESSION_CONTEXT_DEFAULT_WIDTH)
      }}
      className="w-2 bg-transparent after:w-2 focus-visible:ring-inset [&>div]:h-10 [&>div]:bg-[var(--stroke-soft)] hover:[&>div]:bg-[var(--brand-blue)] focus-visible:[&>div]:bg-[var(--brand-blue)]"
    />
  )
  const contextPanel = (
    <ResizablePanel
      key="context"
      id="chat-context"
      panelRef={contextRef}
      defaultSize={width}
      minSize={SESSION_CONTEXT_MIN_WIDTH}
      maxSize={SESSION_CONTEXT_MAX_WIDTH}
      groupResizeBehavior="preserve-pixel-size"
    >
      {context}
    </ResizablePanel>
  )
  const panels = !aside
    ? [conversation]
    : side === 'left'
      ? [contextPanel, separator, conversation]
      : [conversation, separator, contextPanel]

  return (
    <>
      <ResizablePanelGroup
        id="chat-context-layout"
        elementRef={groupElementRef}
        orientation="horizontal"
        className="min-h-0 min-w-0 flex-1"
        disabled={!aside}
        onLayoutChanged={(layout, { isUserInteraction }) => {
          if (!aside || !isUserInteraction) return
          const group = groupElementRef.current
          const handle = handleElementRef.current
          const percent = layout['chat-context']
          if (!group || !handle || !Number.isFinite(percent)) return
          // 键盘回调早于 Panel 的重绘，getSize() 可能仍是旧值；按本次布局计算像素。
          const panelSpace = group.clientWidth - handle.getBoundingClientRect().width
          if (panelSpace > 0) setWidth((panelSpace * percent) / 100)
        }}
      >
        {panels}
      </ResizablePanelGroup>
      {presentation === 'sheet' && context}
    </>
  )
}
