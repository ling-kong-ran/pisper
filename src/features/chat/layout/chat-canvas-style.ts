import { ChatLayoutValidationError } from './chat-layout-error'

export const CANVAS_CSS_MAX_LENGTH = 4096

// 只输出 React 内联声明，不创建样式表；规则、资源请求与宿主页面定位没有入口。
const properties = new Set([
  'display',
  'position',
  'top',
  'right',
  'bottom',
  'left',
  'inset',
  'inset-inline',
  'inset-inline-start',
  'inset-inline-end',
  'inset-block',
  'inset-block-start',
  'inset-block-end',
  'z-index',
  'box-sizing',
  'width',
  'height',
  'min-width',
  'max-width',
  'min-height',
  'max-height',
  'inline-size',
  'block-size',
  'min-inline-size',
  'max-inline-size',
  'min-block-size',
  'max-block-size',
  'aspect-ratio',
  'flex',
  'flex-basis',
  'flex-grow',
  'flex-shrink',
  'flex-direction',
  'flex-wrap',
  'flex-flow',
  'order',
  'align-items',
  'align-self',
  'align-content',
  'justify-items',
  'justify-self',
  'justify-content',
  'place-items',
  'place-self',
  'place-content',
  'gap',
  'row-gap',
  'column-gap',
  'grid',
  'grid-template',
  'grid-template-columns',
  'grid-template-rows',
  'grid-template-areas',
  'grid-auto-columns',
  'grid-auto-rows',
  'grid-auto-flow',
  'grid-column',
  'grid-column-start',
  'grid-column-end',
  'grid-row',
  'grid-row-start',
  'grid-row-end',
  'grid-area',
  'padding',
  'padding-top',
  'padding-right',
  'padding-bottom',
  'padding-left',
  'padding-inline',
  'padding-inline-start',
  'padding-inline-end',
  'padding-block',
  'padding-block-start',
  'padding-block-end',
  'margin',
  'margin-top',
  'margin-right',
  'margin-bottom',
  'margin-left',
  'margin-inline',
  'margin-inline-start',
  'margin-inline-end',
  'margin-block',
  'margin-block-start',
  'margin-block-end',
  'border',
  'border-width',
  'border-style',
  'border-color',
  'border-radius',
  'border-top',
  'border-right',
  'border-bottom',
  'border-left',
  'border-top-width',
  'border-right-width',
  'border-bottom-width',
  'border-left-width',
  'border-top-style',
  'border-right-style',
  'border-bottom-style',
  'border-left-style',
  'border-top-color',
  'border-right-color',
  'border-bottom-color',
  'border-left-color',
  'border-top-left-radius',
  'border-top-right-radius',
  'border-bottom-left-radius',
  'border-bottom-right-radius',
  'border-inline',
  'border-block',
  'border-inline-start',
  'border-inline-end',
  'border-block-start',
  'border-block-end',
  'border-start-start-radius',
  'border-start-end-radius',
  'border-end-start-radius',
  'border-end-end-radius',
  'outline',
  'outline-color',
  'outline-style',
  'outline-width',
  'outline-offset',
  'box-shadow',
  'background',
  'background-color',
  'background-image',
  'background-size',
  'background-position',
  'background-position-x',
  'background-position-y',
  'background-repeat',
  'background-origin',
  'background-clip',
  'background-blend-mode',
  'color',
  'opacity',
  'visibility',
  'font',
  'font-family',
  'font-size',
  'font-weight',
  'font-style',
  'font-stretch',
  'font-variant',
  'font-variant-numeric',
  'font-feature-settings',
  'font-variation-settings',
  'font-kerning',
  'line-height',
  'letter-spacing',
  'word-spacing',
  'text-align',
  'text-align-last',
  'text-indent',
  'text-transform',
  'text-decoration',
  'text-decoration-line',
  'text-decoration-color',
  'text-decoration-style',
  'text-decoration-thickness',
  'text-underline-offset',
  'text-shadow',
  'text-overflow',
  'text-wrap',
  'text-wrap-mode',
  'text-wrap-style',
  'white-space',
  'word-break',
  'overflow-wrap',
  'hyphens',
  'vertical-align',
  'writing-mode',
  'direction',
  'overflow',
  'overflow-x',
  'overflow-y',
  'overflow-inline',
  'overflow-block',
  'overscroll-behavior',
  'overscroll-behavior-x',
  'overscroll-behavior-y',
  'scrollbar-width',
  'scrollbar-color',
  'scrollbar-gutter',
  'object-fit',
  'object-position',
  'filter',
  'backdrop-filter',
  'transform',
  'transform-origin',
  'transform-style',
  'translate',
  'rotate',
  'scale',
  'perspective',
  'perspective-origin',
  'backface-visibility',
  'transition',
  'transition-property',
  'transition-duration',
  'transition-timing-function',
  'transition-delay',
  'cursor',
  'user-select',
  'pointer-events',
  'isolation',
  'contain',
  'float',
  'clear',
  '-webkit-line-clamp',
  '-webkit-box-orient',
  '-webkit-text-fill-color',
  '-webkit-text-stroke',
])

const functions = new Set([
  'rgb',
  'rgba',
  'hsl',
  'hsla',
  'hwb',
  'lab',
  'lch',
  'oklab',
  'oklch',
  'color',
  'color-mix',
  'light-dark',
  'var',
  'calc',
  'min',
  'max',
  'clamp',
  'round',
  'mod',
  'rem',
  'abs',
  'sign',
  'sqrt',
  'pow',
  'log',
  'exp',
  'hypot',
  'sin',
  'cos',
  'tan',
  'asin',
  'acos',
  'atan',
  'atan2',
  'linear-gradient',
  'repeating-linear-gradient',
  'radial-gradient',
  'repeating-radial-gradient',
  'conic-gradient',
  'repeating-conic-gradient',
  'cubic-bezier',
  'steps',
  'repeat',
  'minmax',
  'fit-content',
  'translate',
  'translatex',
  'translatey',
  'translatez',
  'translate3d',
  'scale',
  'scalex',
  'scaley',
  'scalez',
  'scale3d',
  'rotate',
  'rotatex',
  'rotatey',
  'rotatez',
  'rotate3d',
  'skew',
  'skewx',
  'skewy',
  'matrix',
  'matrix3d',
  'perspective',
  'blur',
  'brightness',
  'contrast',
  'drop-shadow',
  'grayscale',
  'hue-rotate',
  'invert',
  'opacity',
  'saturate',
  'sepia',
])

function invalid(): never {
  throw new ChatLayoutValidationError('invalid_css', 'canvas.css')
}

function splitDeclarations(text: string): string[] {
  const declarations: string[] = []
  let start = 0
  let quote = ''
  const brackets: string[] = []
  for (let index = 0; index < text.length; index++) {
    const character = text[index]
    if (quote) {
      if (character === quote) quote = ''
      continue
    }
    if (character === '"' || character === "'") quote = character
    else if (character === '(' || character === '[') {
      brackets.push(character)
      if (brackets.length > 16) invalid()
    } else if (character === ')' || character === ']') {
      if (brackets.pop() !== (character === ')' ? '(' : '[')) invalid()
    } else if (character === ';' && brackets.length === 0) {
      declarations.push(text.slice(start, index))
      start = index + 1
    }
  }
  if (quote || brackets.length) invalid()
  declarations.push(text.slice(start))
  return declarations
}

function validateValue(value: string) {
  if (!value || /[!{}<>@\\`]/.test(value) || /(?:javascript|vbscript|data|https?):/i.test(value))
    invalid()
  for (const match of value.matchAll(/([a-z][a-z0-9-]*)\s*\(/gi)) {
    if (!functions.has(match[1].toLowerCase())) invalid()
  }
  // 冒号只能属于带引号的普通文本，不能引入另一条声明或浏览器私有行为。
  let quote = ''
  for (const character of value) {
    if (quote) {
      if (character === quote) quote = ''
    } else if (character === '"' || character === "'") quote = character
    else if (character === ':' || character === ';') invalid()
  }
}

function styleProperty(property: string) {
  if (property.startsWith('--')) return property
  return property.replace(/-([a-z])/g, (_, letter: string) => letter.toUpperCase())
}

export function parseCanvasCss(text: string): Record<string, string> {
  if (typeof text !== 'string' || text.length > CANVAS_CSS_MAX_LENGTH) invalid()
  for (const character of text) {
    if (/\p{Cc}/u.test(character) && !['\n', '\r', '\t'].includes(character)) invalid()
  }
  // 拒绝转义与注释，避免 u/**/rl、转义函数名等在编辑器与浏览器之间产生不同解释。
  if (/[{}<>@\\`]/.test(text) || text.includes('/*') || text.includes('*/')) invalid()
  const style: Record<string, string> = {}
  for (const declaration of splitDeclarations(text)) {
    if (!declaration.trim()) continue
    const colon = declaration.indexOf(':')
    if (colon < 1) invalid()
    const sourceProperty = declaration.slice(0, colon).trim()
    const custom = /^--[a-zA-Z][a-zA-Z0-9-]{0,63}$/.test(sourceProperty)
    const property = custom ? sourceProperty : sourceProperty.toLowerCase()
    if (!custom && !properties.has(property)) invalid()
    const value = declaration.slice(colon + 1).trim()
    validateValue(value)
    // position 不能通过变量或 revert 取回越出画布的定位方式。
    if (property === 'position' && !['static', 'relative', 'sticky'].includes(value.toLowerCase()))
      invalid()
    style[styleProperty(property)] = value
  }
  return style
}

/** 编辑器修改单条属性时保留引号内的分号，且不把尚未通过校验的草稿作为声明重写。 */
export function splitCanvasCssDeclarations(text: string): string[] {
  parseCanvasCss(text)
  return splitDeclarations(text)
    .map((declaration) => declaration.trim())
    .filter(Boolean)
}
