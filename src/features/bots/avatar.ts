// Bot 头像：12 个角色 × 6 种颜色 × 3 种状态（idle 空闲 / running 干活 / need 需要你）。
// 生成纯 SVG + SMIL 动画（不依赖外部 CSS，不需要放宽 CSP，Safari 16 可用），以 data: URL 交给 <img>，
// 不经 innerHTML；内容只来自下面的固定表与数值，不含任何用户输入。减少动效时输出静态版。
// 设计稿：docs/design/drafts（头像图鉴）。

export const BOT_CHARACTERS = [
  'blob',
  'bean',
  'robot',
  'cat',
  'owl',
  'ghost',
  'cloud',
  'octo',
  'bunny',
  'bear',
  'fox',
  'star',
] as const
export const BOT_COLORS = ['blue', 'green', 'orange', 'purple', 'teal', 'pink'] as const

export type BotCharacter = (typeof BOT_CHARACTERS)[number]
export type BotColor = (typeof BOT_COLORS)[number]
export type BotAvatarState = 'idle' | 'running' | 'need'
export type BotAvatarValue = { character: BotCharacter; color: BotColor }

type Palette = { body: string; shade: string; tile: string }
const PALETTES: Record<BotColor, Palette> = {
  blue: { body: '#8fb4ff', shade: '#5b86e5', tile: '#e9f0ff' },
  green: { body: '#8ed9a8', shade: '#4fb27a', tile: '#e6f6ec' },
  orange: { body: '#ffc287', shade: '#f0954a', tile: '#fff0e2' },
  purple: { body: '#c7a6ff', shade: '#9a73e6', tile: '#f2ebff' },
  teal: { body: '#85d6d6', shade: '#43a9ab', tile: '#e3f5f5' },
  pink: { body: '#ffaccd', shade: '#ea7aa8', tile: '#ffe9f2' },
}
const INK = '#1f2430'
const BLUSH = '#ff7a9c'

type BodyOptions = { state: BotAvatarState; animate: boolean }
type Character = {
  face: [number, number, number]
  body: (options: BodyOptions) => string
  owl?: boolean
  nose?: boolean
  float?: number
}

function anim(attribute: string, values: string, dur: string, keyTimes = '', begin = '0s') {
  return `<animate attributeName="${attribute}" values="${values}" dur="${dur}" begin="${begin}" repeatCount="indefinite"${keyTimes ? ` keyTimes="${keyTimes}"` : ''}/>`
}

function animT(type: string, values: string, dur: string, keyTimes = '', begin = '0s') {
  return `<animateTransform attributeName="transform" type="${type}" values="${values}" dur="${dur}" begin="${begin}" repeatCount="indefinite"${keyTimes ? ` keyTimes="${keyTimes}"` : ''}/>`
}

function starPoints() {
  const points = []
  for (let index = 0; index < 10; index += 1) {
    const radius = index % 2 ? 11.5 : 24
    const angle = (Math.PI / 5) * index - Math.PI / 2
    points.push(
      `${(32 + radius * Math.cos(angle)).toFixed(1)},${(35 + radius * Math.sin(angle)).toFixed(1)}`,
    )
  }
  return points.join(' ')
}

// B / S 是主色与暗色的占位，按颜色替换。
const CHARACTERS: Record<BotCharacter, Character> = {
  blob: {
    face: [32, 34, 1],
    body: () =>
      '<path d="M32 11c13 0 22 9.5 22 22.5S45 55 32 55 10 46.5 10 33.5 19 11 32 11z" fill="B"/>',
  },
  bean: {
    face: [32, 31, 1],
    body: () =>
      '<rect x="15" y="10" width="34" height="46" rx="17" fill="B"/><rect x="21" y="44" width="22" height="6" rx="3" fill="S" opacity=".35"/>',
  },
  robot: {
    face: [32, 36, 1],
    body: ({ state, animate }) =>
      `<line x1="32" y1="18" x2="32" y2="10" stroke="S" stroke-width="3" stroke-linecap="round"/><circle cx="32" cy="8.5" r="3.6" fill="S">${animate && state === 'running' ? anim('opacity', '1;.35;1', '1s') : ''}</circle><rect x="8" y="30" width="6" height="11" rx="3" fill="S"/><rect x="50" y="30" width="6" height="11" rx="3" fill="S"/><rect x="12" y="18" width="40" height="36" rx="13" fill="B"/>`,
  },
  cat: {
    face: [32, 37, 1],
    body: () =>
      '<path d="M15 26 17 9 29 19z M49 26 47 9 35 19z" fill="B" stroke="B" stroke-width="5" stroke-linejoin="round"/><path d="M18.5 21 19.5 14 25 18.5z M45.5 21 44.5 14 39 18.5z" fill="S" opacity=".55"/><ellipse cx="32" cy="37" rx="21" ry="18" fill="B"/><path d="M12 40h7M12 44l7-2M52 40h-7M52 44l-7-2" stroke="S" stroke-width="1.4" stroke-linecap="round" opacity=".7"/>',
  },
  owl: {
    face: [32, 32, 1.15],
    owl: true,
    body: () =>
      '<path d="M16 22 15 11 24 17z M48 22 49 11 40 17z" fill="S" stroke="S" stroke-width="3" stroke-linejoin="round"/><ellipse cx="32" cy="35" rx="20" ry="21" fill="B"/><ellipse cx="32" cy="45" rx="11" ry="9" fill="#fff" opacity=".45"/><path d="M30 39h4l-2 3z" fill="S"/>',
  },
  ghost: {
    face: [32, 30, 1],
    float: 2.6,
    body: () =>
      '<path d="M13 31a19 19 0 0 1 38 0v21l-6.3-4-6.3 4-6.4-4-6.3 4-6.4-4-6.3 4z" fill="B"/>',
  },
  cloud: {
    face: [32, 37, 1],
    body: () =>
      '<circle cx="22" cy="36" r="12" fill="B"/><circle cx="34" cy="29" r="15" fill="B"/><circle cx="45" cy="37" r="11" fill="B"/><rect x="12" y="36" width="42" height="14" rx="7" fill="B"/>',
  },
  octo: {
    face: [32, 29, 1],
    body: ({ animate }) =>
      `<g>${animate ? animT('translate', '0 0;1 0;0 0;-1 0;0 0', '1.8s') : ''}<circle cx="15" cy="45" r="5" fill="S"/><circle cx="23" cy="50" r="5" fill="S"/><circle cx="32" cy="51.5" r="5" fill="S"/><circle cx="41" cy="50" r="5" fill="S"/><circle cx="49" cy="45" r="5" fill="S"/></g><circle cx="32" cy="29" r="19" fill="B"/>`,
  },
  bunny: {
    face: [32, 39, 1],
    body: ({ animate }) =>
      `<g>${animate ? animT('rotate', '0 25 27;0 25 27;-12 25 27;0 25 27', '5.2s', '0;.82;.88;1') : ''}<ellipse cx="25" cy="15" rx="5.5" ry="13" fill="B"/><ellipse cx="25" cy="15" rx="2.5" ry="8.5" fill="S" opacity=".45"/></g><ellipse cx="40" cy="15" rx="5.5" ry="13" fill="B"/><ellipse cx="40" cy="15" rx="2.5" ry="8.5" fill="S" opacity=".45"/><ellipse cx="32" cy="39" rx="19" ry="16" fill="B"/>`,
  },
  bear: {
    face: [32, 35, 1],
    nose: true,
    body: () =>
      '<circle cx="16" cy="20" r="7.5" fill="B"/><circle cx="48" cy="20" r="7.5" fill="B"/><circle cx="16" cy="20" r="3.6" fill="S" opacity=".55"/><circle cx="48" cy="20" r="3.6" fill="S" opacity=".55"/><circle cx="32" cy="36" r="20" fill="B"/><ellipse cx="32" cy="42.5" rx="7.5" ry="5.5" fill="#fff" opacity=".5"/>',
  },
  fox: {
    face: [32, 36, 1],
    nose: true,
    body: () =>
      '<path d="M13 30 15 8 28 20z M51 30 49 8 36 20z" fill="S" stroke="S" stroke-width="4" stroke-linejoin="round"/><path d="M17 22 18 13 24 19z M47 22 46 13 40 19z" fill="#fff" opacity=".7"/><ellipse cx="32" cy="36" rx="21" ry="17" fill="B"/><path d="M14 38q9 13 18 13t18-13q-9 4-18 4t-18-4z" fill="#fff" opacity=".55"/>',
  },
  star: {
    face: [32, 35, 0.85],
    body: () =>
      `<polygon points="${starPoints()}" fill="B" stroke="B" stroke-width="7" stroke-linejoin="round"/>`,
  },
}

// 每个角色的眨眼错开一点，几个头像并排时不会同时眨。
function blinkOffset(character: BotCharacter) {
  let hash = 0
  for (const char of character) hash = (hash * 31 + char.charCodeAt(0)) % 997
  return `${(hash % 23) / 10}s`
}

function face(character: Character, id: BotCharacter, state: BotAvatarState, animate: boolean) {
  const [cx, cy, scale] = character.face
  const blink = (ry: number) =>
    animate
      ? anim('ry', `${ry};${ry};.35;${ry};${ry}`, '4.6s', '0;.9;.94;.98;1', blinkOffset(id))
      : ''
  const eye = (x: number) =>
    character.owl
      ? `<circle cx="${x}" cy="0" r="7.5" fill="#fff"/><ellipse cx="${x}" cy="0" rx="3.2" ry="3.6" fill="${INK}">${blink(3.6)}</ellipse><circle cx="${x + 1.2}" cy="-1.3" r="1.1" fill="#fff"/>`
      : `<ellipse cx="${x}" cy="0" rx="2.6" ry="3.3" fill="${INK}">${blink(3.3)}</ellipse><circle cx="${x + 0.9}" cy="-1.2" r=".9" fill="#fff"/>`
  const look =
    animate && state === 'running'
      ? animT('translate', '0 0;-1.6 0;-1.6 0;1.6 0;1.6 0;0 0', '3s', '0;.2;.4;.6;.8;1')
      : ''
  const mouthY = character.owl ? 9 : 6
  const mouth =
    state === 'need'
      ? `<ellipse cx="0" cy="${mouthY + 0.5}" rx="2.2" ry="2.6" fill="${INK}"/>`
      : `<path d="M-3 ${mouthY}q3 2.6 6 0" fill="none" stroke="${INK}" stroke-width="1.6" stroke-linecap="round"/>`
  const nose = character.nose ? `<ellipse cx="0" cy="4" rx="2" ry="1.4" fill="${INK}"/>` : ''
  return `<g transform="translate(${cx} ${cy}) scale(${scale})"><ellipse cx="-13" cy="5" rx="3.6" ry="2.2" fill="${BLUSH}" opacity=".42"/><ellipse cx="13" cy="5" rx="3.6" ry="2.2" fill="${BLUSH}" opacity=".42"/><g>${look}${eye(-8)}${eye(8)}</g>${nose}${character.nose ? `<g transform="translate(0 2)">${mouth}</g>` : mouth}</g>`
}

// 干活时头顶右边一颗小星星在转，所有角色通用。
function sparkle(palette: Palette, animate: boolean) {
  return `<g transform="translate(52 12)"><path d="M0-6 1.6-1.6 6 0 1.6 1.6 0 6-1.6 1.6-6 0-1.6-1.6z" fill="${palette.shade}">${animate ? animT('rotate', '0;90;180', '1.6s') + anim('opacity', '1;.4;1', '1.6s') : ''}</path></g>`
}

export function botAvatarSvg(
  { character, color }: BotAvatarValue,
  { state = 'idle', animate = true }: { state?: BotAvatarState; animate?: boolean } = {},
) {
  const shape = CHARACTERS[character] ?? CHARACTERS.blob
  const palette = PALETTES[color] ?? PALETTES.blue
  const body = shape
    .body({ state, animate })
    .replaceAll('"B"', `"${palette.body}"`)
    .replaceAll('"S"', `"${palette.shade}"`)
  const motion = !animate
    ? ''
    : state === 'need'
      ? animT('translate', '0 0;0 -4.5;0 0;0 -2;0 0;0 0', '1.3s', '0;.22;.42;.56;.7;1')
      : animT('translate', `0 0;0 -${shape.float ?? 1.2};0 0`, shape.float ? '2.6s' : '3.4s')
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="18" fill="${palette.tile}"/><g transform="translate(32 33) scale(1.12) translate(-32 -33)"><g>${motion}${body}${face(shape, character, state, animate)}</g></g>${state === 'running' ? sparkle(palette, animate) : ''}</svg>`
}

export function botAvatarDataUrl(
  value: BotAvatarValue,
  options?: { state?: BotAvatarState; animate?: boolean },
) {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(botAvatarSvg(value, options))}`
}

export function isBotCharacter(value: unknown): value is BotCharacter {
  return (BOT_CHARACTERS as readonly unknown[]).includes(value)
}

export function isBotColor(value: unknown): value is BotColor {
  return (BOT_COLORS as readonly unknown[]).includes(value)
}

// 新建 Bot 时随机给一个样子：不用先挑头像就能开始。
export function randomBotAvatar(): BotAvatarValue {
  return {
    character: BOT_CHARACTERS[Math.floor(Math.random() * BOT_CHARACTERS.length)],
    color: BOT_COLORS[Math.floor(Math.random() * BOT_COLORS.length)],
  }
}
