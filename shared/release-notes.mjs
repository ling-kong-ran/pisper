function decodeHtmlEntities(value) {
  const named = {
    amp: '&',
    apos: "'",
    gt: '>',
    lt: '<',
    nbsp: ' ',
    quot: '"',
  }
  return String(value || '').replace(
    /&(#x[\da-f]+|#\d+|amp|apos|gt|lt|nbsp|quot);/gi,
    (match, entity) => {
      const normalized = entity.toLowerCase()
      if (normalized.startsWith('#x'))
        return String.fromCodePoint(Number.parseInt(normalized.slice(2), 16))
      if (normalized.startsWith('#'))
        return String.fromCodePoint(Number.parseInt(normalized.slice(1), 10))
      return named[normalized] || match
    },
  )
}

function inlineMarkdown(value) {
  return decodeHtmlEntities(
    String(value || '')
      .replace(/<br\s*\/?\s*>/gi, '\n')
      .replace(
        /<a\b[^>]*href=(['"])(.*?)\1[^>]*>([\s\S]*?)<\/a>/gi,
        (_match, _quote, href, label) => {
          const text = inlineMarkdown(label).trim()
          const target = decodeHtmlEntities(href).trim()
          return /^https?:\/\//i.test(target) ? `[${text || target}](${target})` : text
        },
      )
      .replace(
        /<(strong|b)\b[^>]*>([\s\S]*?)<\/\1>/gi,
        (_match, _tag, content) => `**${inlineMarkdown(content).trim()}**`,
      )
      .replace(
        /<(em|i)\b[^>]*>([\s\S]*?)<\/\1>/gi,
        (_match, _tag, content) => `*${inlineMarkdown(content).trim()}*`,
      )
      .replace(
        /<code\b[^>]*>([\s\S]*?)<\/code>/gi,
        (_match, content) => `\`${decodeHtmlEntities(content).trim()}\``,
      )
      .replace(/<[^>]+>/g, ''),
  )
}

export function normalizeReleaseNotes(value) {
  const source = String(value || '').trim()
  if (!source || !/<\/?[a-z][^>]*>/i.test(source)) return source

  const markdown = source
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(
      /<pre\b[^>]*>([\s\S]*?)<\/pre>/gi,
      (_match, content) =>
        `\n\n\`\`\`\n${decodeHtmlEntities(content.replace(/<[^>]+>/g, '')).trim()}\n\`\`\`\n\n`,
    )
    .replace(
      /<h([1-6])\b[^>]*>([\s\S]*?)<\/h\1>/gi,
      (_match, level, content) =>
        `\n\n${'#'.repeat(Number(level))} ${inlineMarkdown(content).trim()}\n\n`,
    )
    .replace(
      /<li\b[^>]*>([\s\S]*?)<\/li>/gi,
      (_match, content) => `\n- ${inlineMarkdown(content).trim()}`,
    )
    .replace(
      /<blockquote\b[^>]*>([\s\S]*?)<\/blockquote>/gi,
      (_match, content) => `\n\n> ${inlineMarkdown(content).trim().replace(/\n/g, '\n> ')}\n\n`,
    )
    .replace(
      /<p\b[^>]*>([\s\S]*?)<\/p>/gi,
      (_match, content) => `\n\n${inlineMarkdown(content).trim()}\n\n`,
    )
    .replace(/<br\s*\/?\s*>/gi, '\n')
    .replace(/<\/?(?:div|section|article|ul|ol|details|summary)\b[^>]*>/gi, '\n')
    .replace(/<[^>]+>/g, '')

  return decodeHtmlEntities(markdown)
    .replace(/\r/g, '')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n[ \t]+/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}

export function releaseNotesMarkdown(notes) {
  const values = Array.isArray(notes)
    ? notes.map((item) => (typeof item === 'string' ? item : item?.note || ''))
    : [typeof notes === 'string' ? notes : '']
  return values.map(normalizeReleaseNotes).filter(Boolean).join('\n\n')
}

export function hasMeaningfulGeneratedNotes(body) {
  const content = String(body || '')
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/^\s*#{1,6}\s*(?:what['’]?s changed|更新内容)\s*$/gim, '')
    .replace(/^\s*\*\*(?:full changelog|完整变更)\*\*\s*[:：].*$/gim, '')
    .replace(/^\s*(?:full changelog|完整变更)\s*[:：].*$/gim, '')
    .trim()
  return Boolean(content)
}
