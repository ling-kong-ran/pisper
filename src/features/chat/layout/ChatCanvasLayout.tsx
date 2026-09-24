import {
  createContext,
  memo,
  useContext,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'
import { CanvasCustomUi } from './CanvasCustomUi'
import type { ChatCanvasNode } from './chat-canvas'
import {
  CHAT_CANVAS_SLOT_KINDS,
  chatCanvasNodeStyle,
  createChatCanvasHosts,
  isChatCanvasSlotKind,
  isFloatingCanvasIsland,
  type ChatCanvasSlotKind,
} from './chat-canvas-render'

type ChatCanvasSlots = Partial<Record<ChatCanvasSlotKind, ReactNode>>
const EMPTY_CANVAS_SLOTS: ChatCanvasSlots = {}
type CanvasFocusSnapshot = {
  element: HTMLElement
  selection: { start: number; end: number; direction: 'forward' | 'backward' | 'none' } | null
}
type CanvasContextValue = {
  slots: ChatCanvasSlots
  getHost: ((kind: ChatCanvasSlotKind) => HTMLDivElement) | null
  focusSnapshots: Map<ChatCanvasSlotKind, CanvasFocusSnapshot>
  notify?: (message: string) => void
}

const CanvasContext = createContext<CanvasContextValue | null>(null)

export function ChatCanvasSlot({ kind }: { kind: ChatCanvasSlotKind }) {
  const context = useContext(CanvasContext)
  const anchorRef = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    const anchor = anchorRef.current
    const host = context?.getHost?.(kind)
    if (!anchor || !host || !context) return
    if (anchor.firstChild !== host || anchor.childNodes.length !== 1) anchor.replaceChildren(host)
    const previous = context.focusSnapshots.get(kind)
    context.focusSnapshots.delete(kind)
    if (
      previous?.element.isConnected &&
      (document.activeElement === document.body || document.activeElement === previous.element)
    ) {
      previous.element.focus({ preventScroll: true })
      if (
        previous.selection &&
        (previous.element instanceof HTMLInputElement ||
          previous.element instanceof HTMLTextAreaElement)
      ) {
        previous.element.setSelectionRange(
          previous.selection.start,
          previous.selection.end,
          previous.selection.direction,
        )
      }
    }
    return () => {
      // 在旧锚点被移除前保存；新父节点的 layout effect 再恢复，避免提交后才读到 body。
      const element = document.activeElement
      if (
        host.parentElement !== anchor ||
        !(element instanceof HTMLElement) ||
        !host.contains(element)
      )
        return
      const field =
        element instanceof HTMLTextAreaElement || element instanceof HTMLInputElement
          ? element
          : null
      context.focusSnapshots.set(kind, {
        element,
        selection:
          field?.selectionStart != null && field.selectionEnd != null
            ? {
                start: field.selectionStart,
                end: field.selectionEnd,
                direction: field.selectionDirection ?? 'none',
              }
            : null,
      })
    }
  })
  if (!context) return null
  if (!context.getHost) return context.slots[kind]
  return <div ref={anchorRef} className="contents" data-canvas-slot={kind} />
}

const CanvasNode = memo(function CanvasNode({
  node,
  root = false,
}: {
  node: ChatCanvasNode
  root?: boolean
}) {
  const context = useContext(CanvasContext)
  if (isFloatingCanvasIsland(node)) return null
  return (
    <div
      data-canvas-node={node.id}
      data-canvas-kind={node.kind}
      style={chatCanvasNodeStyle(node, root)}
      role={node.kind === 'divider' ? 'separator' : undefined}
      aria-hidden={node.kind === 'spacer' || undefined}
    >
      {isChatCanvasSlotKind(node.kind) ? (
        <ChatCanvasSlot kind={node.kind} />
      ) : node.kind === 'text' ? (
        node.text
      ) : node.kind === 'custom-ui' && node.componentId ? (
        <CanvasCustomUi componentId={node.componentId} notify={context?.notify} />
      ) : (
        node.children?.map((child) => <CanvasNode key={child.id} node={child} />)
      )}
    </div>
  )
})

export function ChatCanvasLayout({
  root,
  slots,
  notify,
}: {
  root: ChatCanvasNode
  slots: ChatCanvasSlots
  notify?: (message: string) => void
}) {
  const [getHost] = useState(() =>
    typeof document === 'undefined'
      ? null
      : createChatCanvasHosts((kind) => {
          const host = document.createElement('div')
          host.dataset.canvasHost = kind
          host.style.display = 'contents'
          return host
        }),
  )
  const fallbackSlots = getHost ? EMPTY_CANVAS_SLOTS : slots
  const [focusSnapshots] = useState(() => new Map<ChatCanvasSlotKind, CanvasFocusSnapshot>())
  const context = useMemo(
    () => ({ slots: fallbackSlots, getHost, focusSnapshots, notify }),
    [getHost, fallbackSlots, focusSnapshots, notify],
  )
  return (
    <CanvasContext.Provider value={context}>
      <div className="flex h-full min-h-0 min-w-0 flex-1 overflow-auto" data-chat-canvas>
        <CanvasNode node={root} root />
      </div>
      {getHost &&
        CHAT_CANVAS_SLOT_KINDS.map((kind) =>
          slots[kind] == null ? null : createPortal(slots[kind], getHost(kind), kind),
        )}
    </CanvasContext.Provider>
  )
}
