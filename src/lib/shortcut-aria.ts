// aria-keyshortcuts 要求按 KeyboardEvent.key 的写法列出组合键（“Control+Alt+J”“Meta+/”），
// 与界面上给人看的“Ctrl + Alt + J”不同。只给按需加载的页面使用，不进入入口包。
const usesCommandKey = /Mac|iPhone|iPad/.test(globalThis.navigator?.platform || '')

const KEY_VALUES: Record<string, string> = {
  Backquote: '`',
  Minus: '-',
  Equal: '=',
  BracketLeft: '[',
  BracketRight: ']',
  Backslash: '\\',
  Semicolon: ';',
  Quote: "'",
  Comma: ',',
  Period: '.',
  Slash: '/',
}

// 绑定已由 shortcut-store 校验过；未绑定时返回 undefined，让属性整个省略。
export function ariaKeyShortcuts(binding: string | null | undefined, mac = usesCommandKey) {
  if (!binding) return undefined
  const parts = binding.split('+')
  const code = parts.pop() || ''
  const key = /^(Key[A-Z]|Digit\d)$/.test(code) ? code.slice(-1) : (KEY_VALUES[code] ?? code)
  const modifiers = parts.map((part) =>
    part === 'Mod' ? (mac ? 'Meta' : 'Control') : part === 'Ctrl' ? 'Control' : part,
  )
  return [...modifiers, key].join('+')
}
