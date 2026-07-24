function stripRepeatedLead(lead: unknown, body: unknown) {
  let next = String(body || '')
  const frozen = String(lead || '')
  if (!frozen || !next) return next

  // Exact restated preamble.
  if (next.startsWith(frozen)) next = next.slice(frozen.length).replace(/^\s+/, '')

  // Body restarts from a long suffix/prefix already shown in the lead.
  const min = 16
  const max = Math.min(frozen.length, next.length)
  for (let len = max; len >= min; len -= 1) {
    const suffix = frozen.slice(-len)
    if (next.startsWith(suffix)) return next.slice(len).replace(/^\s+/, '')
  }
  for (let len = max; len >= min; len -= 1) {
    const prefix = next.slice(0, len)
    if (frozen.endsWith(prefix)) return next.slice(len).replace(/^\s+/, '')
  }
  return next
}

function hasSubstantialOverlap(lead: unknown, body: unknown) {
  const left = String(lead || '')
  const right = String(body || '')
  if (left.length < 16 || right.length < 16) return false
  // Try progressively shorter prefixes so a restated sentence still matches.
  const max = Math.min(48, right.length)
  for (let len = max; len >= 16; len -= 1) {
    if (left.includes(right.slice(0, len))) return true
  }
  return false
}

/**
 * Split accumulated assistant text around tool calls for display.
 * - lead: frozen preamble captured when tools first start
 * - body: remaining/new text after that point
 *
 * If the model restates earlier content after tools, prefer a single body block
 * so the UI does not show the same paragraph above and below the tool panel.
 */
export function splitAssistantStreamText(
  fullText: unknown,
  preamble: unknown,
  { streaming = false, hasTools = false }: { streaming?: boolean; hasTools?: boolean } = {},
) {
  const text = String(fullText || '')
  if (!streaming || !hasTools) {
    return { lead: '', body: text, mode: 'single' }
  }

  const frozen = String(preamble || '')
  if (!frozen) {
    return { lead: '', body: text, mode: 'single' }
  }
  if (!text) {
    return { lead: frozen, body: '', mode: 'split' }
  }

  // Typewriter still catching up inside the preamble.
  if (frozen.startsWith(text) && text.length < frozen.length) {
    return { lead: text, body: '', mode: 'split' }
  }

  if (text.startsWith(frozen)) {
    const body = stripRepeatedLead(frozen, text.slice(frozen.length).replace(/^\s+/, ''))
    if (!body) return { lead: frozen, body: '', mode: 'split' }
    if (hasSubstantialOverlap(frozen, body)) {
      // Post-tool text restates the preamble idea: show one combined answer under tools.
      return { lead: '', body: text, mode: 'single' }
    }
    return { lead: frozen, body, mode: 'split' }
  }

  // Target text no longer begins with the frozen preamble (rewrite / patch).
  return { lead: '', body: text, mode: 'single' }
}
