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
  const expression = new RegExp(
    `(?<![A-Za-z0-9_./\\\\-])(?:${patterns.join('|')})(?![A-Za-z0-9_/\\\\-]|\\.[A-Za-z0-9_])`,
    'gi',
  )
  return text.replace(
    expression,
    (match) => replacements.get(match.toLowerCase().replace(/[ \t]+/g, ' ')) || match,
  )
}
