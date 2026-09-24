import { createDefaultCanvas, parseChatCanvas, type ChatCanvasNode } from './chat-canvas'
import { ChatLayoutValidationError } from './chat-layout-error'
import { createStudioCanvas } from './chat-layout-presets'
export { ChatLayoutValidationError } from './chat-layout-error'
export type { ChatLayoutValidationCode } from './chat-layout-error'

export type ChatLayoutAppearance = {
  composerPosition: 'top' | 'bottom'
  contentWidth: number
  fontSize: number | null
  fontFamily: 'inherit' | 'system' | 'serif'
  density: 'comfortable' | 'compact'
  messageStyle: 'bubble' | 'plain'
  showUsage: boolean
}

export type DesktopChatLayout = ChatLayoutAppearance & {
  canvas: ChatCanvasNode
  navigationSide: 'left' | 'right'
  navigationWidth: number
  navigationCollapsed: boolean | null
  contextSide: 'left' | 'right'
  contextVisibility: 'auto' | 'closed'
  contextWidth: number
  openContextOnCompletion: boolean
}

export type MobileChatLayout = ChatLayoutAppearance & {
  canvas: ChatCanvasNode
  openContextOnCompletion: boolean
}

export type ChatLayoutTemplate = {
  version: 2
  name: string
  accent: 'inherit' | 'blue' | 'teal' | 'violet'
  desktop: DesktopChatLayout
  mobile: MobileChatLayout
}

export const CHAT_LAYOUT_MAX_BYTES = 64 * 1024
export const CHAT_LAYOUT_SAVED_LIMIT = 20

const appearance: ChatLayoutAppearance = {
  composerPosition: 'bottom',
  contentWidth: 1040,
  fontSize: null,
  fontFamily: 'inherit',
  density: 'comfortable',
  messageStyle: 'bubble',
  showUsage: true,
}

function freezeCanvas(node: ChatCanvasNode): ChatCanvasNode {
  for (const child of node.children ?? []) freezeCanvas(child)
  if (node.children) Object.freeze(node.children)
  return Object.freeze(node)
}

export const DEFAULT_CHAT_LAYOUT: ChatLayoutTemplate = Object.freeze({
  version: 2,
  name: 'Default',
  accent: 'inherit',
  desktop: Object.freeze({
    ...appearance,
    canvas: freezeCanvas(createDefaultCanvas(appearance)),
    navigationSide: 'left',
    navigationWidth: 236,
    navigationCollapsed: null,
    contextSide: 'right',
    contextVisibility: 'auto',
    contextWidth: 360,
    openContextOnCompletion: true,
  }),
  mobile: Object.freeze({
    ...appearance,
    canvas: freezeCanvas(createDefaultCanvas(appearance)),
    openContextOnCompletion: true,
  }),
})

export type ChatLayoutPreset = {
  id: 'default' | 'focus' | 'workbench' | 'studio'
  template: ChatLayoutTemplate
}

export const CHAT_LAYOUT_PRESETS: readonly ChatLayoutPreset[] = Object.freeze([
  Object.freeze({ id: 'default', template: DEFAULT_CHAT_LAYOUT }),
  Object.freeze({
    id: 'focus',
    template: Object.freeze({
      ...DEFAULT_CHAT_LAYOUT,
      name: 'Focus',
      desktop: Object.freeze({
        ...DEFAULT_CHAT_LAYOUT.desktop,
        contentWidth: 880,
        navigationCollapsed: true,
        contextVisibility: 'closed',
        openContextOnCompletion: false,
        messageStyle: 'plain',
      }),
      mobile: Object.freeze({
        ...DEFAULT_CHAT_LAYOUT.mobile,
        messageStyle: 'plain',
        openContextOnCompletion: false,
      }),
    }),
  }),
  Object.freeze({
    id: 'workbench',
    template: Object.freeze({
      ...DEFAULT_CHAT_LAYOUT,
      name: 'Workbench',
      desktop: Object.freeze({
        ...DEFAULT_CHAT_LAYOUT.desktop,
        contentWidth: 1200,
        navigationCollapsed: false,
        contextSide: 'left',
        contextWidth: 440,
        density: 'compact',
      }),
      mobile: Object.freeze({ ...DEFAULT_CHAT_LAYOUT.mobile, density: 'compact' }),
    }),
  }),
  Object.freeze({
    id: 'studio',
    template: Object.freeze({
      ...DEFAULT_CHAT_LAYOUT,
      name: 'Studio',
      desktop: Object.freeze({
        ...DEFAULT_CHAT_LAYOUT.desktop,
        canvas: freezeCanvas(createStudioCanvas(false)),
        contentWidth: 960,
        fontSize: 15,
        fontFamily: 'system',
        navigationWidth: 216,
        contextSide: 'right',
        contextVisibility: 'auto',
        contextWidth: 340,
      }),
      mobile: Object.freeze({
        ...DEFAULT_CHAT_LAYOUT.mobile,
        canvas: freezeCanvas(createStudioCanvas(true)),
        contentWidth: 960,
        fontFamily: 'system',
      }),
    }),
  }),
])

function equalLayoutValue(left: unknown, right: unknown): boolean {
  if (left === right) return true
  if (typeof left !== 'object' || left === null || typeof right !== 'object' || right === null)
    return false
  if (Array.isArray(left) || Array.isArray(right))
    return (
      Array.isArray(left) &&
      Array.isArray(right) &&
      left.length === right.length &&
      left.every((value, index) => equalLayoutValue(value, right[index]))
    )
  const leftEntries = Object.entries(left)
  const rightEntries = Object.entries(right)
  return (
    leftEntries.length === rightEntries.length &&
    leftEntries.every(([key, value]) =>
      rightEntries.some(
        ([otherKey, otherValue]) => key === otherKey && equalLayoutValue(value, otherValue),
      ),
    )
  )
}

/** 模板显示名可以本地化或重命名；匹配预设时只比较实际配置，忽略对象属性插入顺序。 */
export function equalChatLayoutContent(
  left: ChatLayoutTemplate,
  right: ChatLayoutTemplate,
): boolean {
  return (
    left.version === right.version &&
    left.accent === right.accent &&
    equalLayoutValue(left.desktop, right.desktop) &&
    equalLayoutValue(left.mobile, right.mobile)
  )
}

function object(value: unknown, fields: readonly string[], path: string) {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new ChatLayoutValidationError('invalid_type', path)
  const prototype: unknown = Object.getPrototypeOf(value)
  if (prototype !== Object.prototype && prototype !== null)
    throw new ChatLayoutValidationError('invalid_type', path)
  for (const key of Reflect.ownKeys(value)) {
    if (typeof key !== 'string' || !fields.includes(key))
      throw new ChatLayoutValidationError('unknown_field', path)
  }
  const result: Record<string, unknown> = {}
  for (const field of fields) {
    const descriptor = Object.getOwnPropertyDescriptor(value, field)
    if (!descriptor) throw new ChatLayoutValidationError('missing_field', `${path}.${field}`)
    if (!('value' in descriptor))
      throw new ChatLayoutValidationError('invalid_type', `${path}.${field}`)
    result[field] = descriptor.value
  }
  return result
}

function choice<T extends string>(value: unknown, options: readonly T[], path: string): T {
  for (const option of options) if (value === option) return option
  throw new ChatLayoutValidationError('invalid_value', path)
}

function number(value: unknown, minimum: number, maximum: number, path: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value))
    throw new ChatLayoutValidationError('invalid_type', path)
  if (value < minimum || value > maximum) throw new ChatLayoutValidationError('out_of_range', path)
  return value
}

function boolean(value: unknown, path: string): boolean {
  if (typeof value !== 'boolean') throw new ChatLayoutValidationError('invalid_type', path)
  return value
}

function parseAppearance(value: Record<string, unknown>, path: string): ChatLayoutAppearance {
  return {
    composerPosition: choice(value.composerPosition, ['top', 'bottom'], `${path}.composerPosition`),
    contentWidth: number(value.contentWidth, 600, 1600, `${path}.contentWidth`),
    fontSize: value.fontSize === null ? null : number(value.fontSize, 13, 20, `${path}.fontSize`),
    fontFamily: choice(value.fontFamily, ['inherit', 'system', 'serif'], `${path}.fontFamily`),
    density: choice(value.density, ['comfortable', 'compact'], `${path}.density`),
    messageStyle: choice(value.messageStyle, ['bubble', 'plain'], `${path}.messageStyle`),
    showUsage: boolean(value.showUsage, `${path}.showUsage`),
  }
}

/** v1 只包含固定布局选项；v2 增加受限组件树与节点内联 CSS，旧模板在读取边界迁移。 */
export function parseChatLayout(input: unknown): ChatLayoutTemplate {
  const template = object(input, ['version', 'name', 'accent', 'desktop', 'mobile'], 'layout')
  if (template.version !== 1 && template.version !== 2)
    throw new ChatLayoutValidationError('unsupported_version', 'version')
  const canvasFields = template.version === 2 ? ['canvas'] : []
  if (typeof template.name !== 'string') throw new ChatLayoutValidationError('invalid_type', 'name')
  const name = template.name.trim()
  if (!name || [...name].length > 80 || /\p{Cc}/u.test(name))
    throw new ChatLayoutValidationError('invalid_value', 'name')
  const desktop = object(
    template.desktop,
    [
      ...Object.keys(appearance),
      ...canvasFields,
      'navigationSide',
      'navigationWidth',
      'navigationCollapsed',
      'contextSide',
      'contextVisibility',
      'contextWidth',
      'openContextOnCompletion',
    ],
    'desktop',
  )
  const mobile = object(
    template.mobile,
    [...Object.keys(appearance), ...canvasFields, 'openContextOnCompletion'],
    'mobile',
  )
  const desktopAppearance = parseAppearance(desktop, 'desktop')
  const mobileAppearance = parseAppearance(mobile, 'mobile')
  const result: ChatLayoutTemplate = {
    version: 2,
    name,
    accent: choice(template.accent, ['inherit', 'blue', 'teal', 'violet'], 'accent'),
    desktop: {
      ...desktopAppearance,
      canvas:
        template.version === 1
          ? createDefaultCanvas(desktopAppearance)
          : parseChatCanvas(desktop.canvas),
      navigationSide: choice(desktop.navigationSide, ['left', 'right'], 'desktop.navigationSide'),
      navigationWidth: number(desktop.navigationWidth, 200, 320, 'desktop.navigationWidth'),
      navigationCollapsed:
        desktop.navigationCollapsed === null
          ? null
          : boolean(desktop.navigationCollapsed, 'desktop.navigationCollapsed'),
      contextSide: choice(desktop.contextSide, ['left', 'right'], 'desktop.contextSide'),
      contextVisibility: choice(
        desktop.contextVisibility,
        ['auto', 'closed'],
        'desktop.contextVisibility',
      ),
      contextWidth: number(desktop.contextWidth, 280, 720, 'desktop.contextWidth'),
      openContextOnCompletion: boolean(
        desktop.openContextOnCompletion,
        'desktop.openContextOnCompletion',
      ),
    },
    mobile: {
      ...mobileAppearance,
      canvas:
        template.version === 1
          ? createDefaultCanvas(mobileAppearance)
          : parseChatCanvas(mobile.canvas),
      openContextOnCompletion: boolean(
        mobile.openContextOnCompletion,
        'mobile.openContextOnCompletion',
      ),
    },
  }
  // 通过编辑器组合出的模板也必须可导出并重新导入，而不只限制文件导入入口。
  if (new TextEncoder().encode(JSON.stringify(result)).length > CHAT_LAYOUT_MAX_BYTES)
    throw new ChatLayoutValidationError('too_large')
  return result
}

export function parseChatLayoutJson(text: string): ChatLayoutTemplate {
  // 先限制字符数，避免为了计算超大文件的字节数再次分配同样大小的缓冲区。
  if (
    text.length > CHAT_LAYOUT_MAX_BYTES ||
    new TextEncoder().encode(text).length > CHAT_LAYOUT_MAX_BYTES
  )
    throw new ChatLayoutValidationError('too_large')
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch {
    throw new ChatLayoutValidationError('invalid_json')
  }
  return parseChatLayout(value)
}

export function serializeChatLayout(template: ChatLayoutTemplate): string {
  const text = `${JSON.stringify(parseChatLayout(template), null, 2)}\n`
  if (new TextEncoder().encode(text).length > CHAT_LAYOUT_MAX_BYTES)
    throw new ChatLayoutValidationError('too_large')
  return text
}
