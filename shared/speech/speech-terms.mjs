// 仅规范已知术语的拼写，不根据同音关系替换 py、pi 或普通英文单词。
export function spokenTerm(term) {
  return String(term)
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/([A-Z])([A-Z][a-z])/g, '$1 $2')
    .replace(/[_-]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .toLowerCase()
}

export function speechHotwords(terms) {
  return [...new Set(terms.map(spokenTerm).filter(Boolean))].join('\n')
}

// Safari 16.4 以下不支持 lookbehind，构造即抛 “invalid group specifier name”，
// 会把整个语音链路带挂；这里在构造点做特性检测并降级为无 lookbehind 的等价正则，
// 由 replace 回调自己判断前一个字符，行为与原 lookbehind 一致。
let lookbehindSupported
function supportsLookbehind() {
  if (lookbehindSupported === undefined) {
    try {
      new RegExp('(?<=a)')
      lookbehindSupported = true
    } catch {
      lookbehindSupported = false
    }
  }
  return lookbehindSupported
}

const TERM_BOUNDARY_CHARS = /[A-Za-z0-9_./\\-]/

export function formatSpeechTerms(text, terms) {
  const replacements = new Map()
  for (const term of terms) {
    const spoken = spokenTerm(term)
    // 小写命令短语也保留，用最长匹配防止 cargo test 中的 cargo 被单词规则改写。
    if ((!/[A-Z]/.test(term) && !term.includes(' ')) || !/^[A-Za-z][A-Za-z0-9 ]*$/.test(term))
      continue
    for (const variant of [spoken, term.toLowerCase()]) {
      if (variant.length >= 3 && !replacements.has(variant)) replacements.set(variant, term)
    }
  }
  if (!replacements.size) return text
  const patterns = [...replacements.keys()]
    .sort((a, b) => b.length - a.length)
    .map((value) => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&').replace(/ /g, '[ \\t]+'))
  const alternation = `(?:${patterns.join('|')})`
  const trailing = `(?![A-Za-z0-9_/\\\\-]|\\.[A-Za-z0-9_])`
  const useLookbehind = supportsLookbehind()
  const expression = new RegExp(
    useLookbehind
      ? `(?<![A-Za-z0-9_./\\\\-])${alternation}${trailing}`
      : `${alternation}${trailing}`,
    'gi',
  )
  return text.replace(expression, (match, ...rest) => {
    // 降级分支没有 lookbehind，回调需自己排除词内命中；此时 rest 为 [offset, string]。
    if (!useLookbehind) {
      const offset = rest[rest.length - 2]
      const source = rest[rest.length - 1]
      if (offset > 0 && TERM_BOUNDARY_CHARS.test(source[offset - 1])) return match
    }
    return replacements.get(match.toLowerCase().replace(/[ \t]+/g, ' ')) || match
  })
}
