// @public 应用壳与布局编辑入口可读取模板契约和偏好；不导入聊天渲染或会话运行时。
export {
  CHAT_LAYOUT_MAX_BYTES,
  CHAT_LAYOUT_PRESETS,
  CHAT_LAYOUT_SAVED_LIMIT,
  ChatLayoutValidationError,
  DEFAULT_CHAT_LAYOUT,
  equalChatLayoutContent,
  parseChatLayout,
  parseChatLayoutJson,
  serializeChatLayout,
} from '@/features/chat/layout/chat-layout'
export type {
  ChatLayoutAppearance,
  ChatLayoutPreset,
  ChatLayoutTemplate,
  ChatLayoutValidationCode,
  DesktopChatLayout,
  MobileChatLayout,
} from '@/features/chat/layout/chat-layout'
export { useChatLayoutStore } from '@/features/chat/layout/chat-layout-store'
export type { SavedChatLayout } from '@/features/chat/layout/chat-layout-store'
export {
  CANVAS_KINDS,
  CANVAS_CONTAINER_KINDS,
  CHAT_CANVAS_SLOT_KINDS,
  CANVAS_MAX_NODES,
  CANVAS_MAX_DEPTH,
  CANVAS_TEXT_MAX_LENGTH,
  isCanvasContainer,
  isCanvasContainerKind,
  parseChatCanvas,
  createDefaultCanvas,
  findCanvasNode,
  canvasHasKind,
  addCanvasNode,
  moveCanvasNode,
  removeCanvasNode,
  updateCanvasNode,
} from '@/features/chat/layout/chat-canvas'
export type {
  ChatCanvasKind,
  ChatCanvasSlotKind,
  ChatCanvasNode,
} from '@/features/chat/layout/chat-canvas'
export { CANVAS_CSS_MAX_LENGTH, parseCanvasCss } from '@/features/chat/layout/chat-canvas-style'
