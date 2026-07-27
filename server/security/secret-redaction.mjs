import { readFile, readdir, rename, unlink, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

export const REDACTED_SECRET = '[REDACTED SECRET]'

const ALWAYS_SENSITIVE_KEYS = new Set([
  'apikey',
  'appsecret',
  'authorization',
  'authtoken',
  'clientsecret',
  'cookie',
  'credential',
  'credentials',
  'password',
  'passwd',
  'refreshtoken',
  'secret',
  'setcookie',
  'accesstoken',
])

const EXPLICIT_SENSITIVE_KEY_PATTERN = '(?:api[_ -]?key|access[_ -]?token|refresh[_ -]?token|auth[_ -]?token|client[_ -]?secret|app[_ -]?secret|password|passwd|authorization|cookie|credentials?)'
const GENERIC_TOKEN_KEY_PATTERN = '(?<![a-z0-9_])token(?![a-z0-9_])'
const QUOTED_SECRET = new RegExp(`((?:["'])?${EXPLICIT_SENSITIVE_KEY_PATTERN}(?:["'])?\\s*[:=]\\s*)(["'])([^\\r\\n]*?)\\2`, 'gi')
const PLAIN_SECRET = new RegExp(`((?:^|[\\s,{;])(?:${EXPLICIT_SENSITIVE_KEY_PATTERN})\\s*[:=]\\s*)(?!["']|\\[REDACTED SECRET\\])[^\\s,;}\\]]+`, 'gim')
const CLI_SECRET = new RegExp(`(--?(?:${EXPLICIT_SENSITIVE_KEY_PATTERN})(?:=|\\s+))(?!\\[REDACTED SECRET\\])([^\\s"']+)`, 'gi')
const QUOTED_GENERIC_TOKEN = new RegExp(`((?:["'])?${GENERIC_TOKEN_KEY_PATTERN}(?:["'])?\\s*[:=]\\s*)(["'])([^\\r\\n]*?)\\2`, 'gi')
const PLAIN_GENERIC_TOKEN = new RegExp(`((?:^|[\\s,{;])${GENERIC_TOKEN_KEY_PATTERN}\\s*[:=]\\s*)(?!["']|\\[REDACTED SECRET\\])([^\\s,;}\\]]+)`, 'gim')
const CLI_GENERIC_TOKEN = new RegExp(`(--?token(?:=|\\s+))(?!\\[REDACTED SECRET\\])([^\\s"']+)`, 'gi')
const PERSISTENCE_REDACTION = Symbol('pisperPersistenceRedaction')
// Withhold a short tail while streaming so split secrets are not flashed; keep small to avoid obvious truncation.
const STREAM_GUARD_LENGTH = 28
const PRIVATE_KEY_BEGIN = /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/g
const PRIVATE_KEY_END = /-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/g

function normalizedKey(value) {
  return String(value || '').toLowerCase().replace(/[^a-z0-9]/g, '')
}

function looksLikeSecret(value) {
  if (typeof value !== 'string') return false
  const text = value.trim()
  if (!text || text === REDACTED_SECRET) return false
  if (/^Bearer\s+/i.test(text)) return true
  if (/^eyJ[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}$/i.test(text)) return true
  if (/^(?:sk|rk|pk|pcl|ghp|github_pat|xox[baprs])[-_][a-z0-9_-]{12,}$/i.test(text)) return true
  return text.length >= 20 && !/\s/.test(text) && /[a-z]/i.test(text) && /(?:\d|[^a-z0-9])/i.test(text)
}

function sensitiveKey(value, content) {
  const key = normalizedKey(value)
  if (key === 'token') return looksLikeSecret(content)
  return ALWAYS_SENSITIVE_KEYS.has(key) || /(?:apikey|secret|password|passwd|authorization|credential|accesstoken|refreshtoken|authtoken)$/.test(key)
}

export function redactSecretText(value) {
  return String(value ?? '')
    .replace(/-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/g, REDACTED_SECRET)
    .replace(/\b(Bearer\s+)[a-z0-9._~+/-]{8,}/gi, `$1${REDACTED_SECRET}`)
    .replace(QUOTED_SECRET, (_match, prefix, quote) => `${prefix}${quote}${REDACTED_SECRET}${quote}`)
    .replace(PLAIN_SECRET, (_match, prefix) => `${prefix}${REDACTED_SECRET}`)
    .replace(CLI_SECRET, (_match, prefix) => `${prefix}${REDACTED_SECRET}`)
    .replace(QUOTED_GENERIC_TOKEN, (match, prefix, quote, secret) => looksLikeSecret(secret) ? `${prefix}${quote}${REDACTED_SECRET}${quote}` : match)
    .replace(PLAIN_GENERIC_TOKEN, (match, prefix, secret) => looksLikeSecret(secret) ? `${prefix}${REDACTED_SECRET}` : match)
    .replace(CLI_GENERIC_TOKEN, (match, prefix, secret) => looksLikeSecret(secret) ? `${prefix}${REDACTED_SECRET}` : match)
    .replace(/([?&](?:(?:access|refresh|auth)[_-]?)?(?:token|key|secret|password|auth|credential)[^=&#\s]*=)(?!\[REDACTED SECRET\])[^&#\s]*/gi, `$1${REDACTED_SECRET}`)
    .replace(/\beyJ[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}\b/gi, REDACTED_SECRET)
    .replace(/\b(?:sk|rk|pk|pcl|ghp|github_pat|xox[baprs])[-_][a-z0-9_-]{12,}\b/gi, REDACTED_SECRET)
}

export function redactSecretValue(value, key = '', seen = new WeakSet()) {
  if (sensitiveKey(key, value)) return value == null ? value : REDACTED_SECRET
  if (typeof value === 'string') return redactSecretText(value)
  if (!value || typeof value !== 'object') return value
  if (seen.has(value)) return '[CIRCULAR]'
  seen.add(value)
  const redacted = Array.isArray(value)
    ? value.map((item) => redactSecretValue(item, key, seen))
    : Object.fromEntries(
        Object.entries(value).map(([childKey, child]) => [
          childKey,
          redactSecretValue(child, childKey, seen),
        ]),
      )
  seen.delete(value)
  return redacted
}

function commonPrefixLength(left, right) {
  const limit = Math.min(left.length, right.length)
  let index = 0
  while (index < limit && left.charCodeAt(index) === right.charCodeAt(index)) index += 1
  return index
}

function unmatchedPrivateKeyStart(value) {
  const begins = [...value.matchAll(PRIVATE_KEY_BEGIN)]
  const ends = [...value.matchAll(PRIVATE_KEY_END)]
  if (begins.length <= ends.length) return -1
  return begins.at(-1)?.index ?? -1
}

export function createStreamingSecretRedactor({ guardLength = STREAM_GUARD_LENGTH } = {}) {
  let source = ''
  let visible = ''

  const patchFor = (nextVisible) => {
    if (nextVisible === visible) return null
    const start = commonPrefixLength(visible, nextVisible)
    visible = nextVisible
    return { start, text: nextVisible.slice(start) }
  }

  const render = (final) => {
    const privateKeyStart = unmatchedPrivateKeyStart(source)
    const redacted = privateKeyStart >= 0
      ? redactSecretText(source.slice(0, privateKeyStart))
      : redactSecretText(source)
    const guardedLength = Math.max(0, redacted.length - Math.max(0, guardLength))
    // Once a completed content block has been flushed, later blocks may continue
    // streaming through the same redactor. Do not retract that already-safe prefix
    // merely to recreate the guard window; still allow rewrites when new input makes
    // the previously visible text part of a secret spanning a block boundary.
    const stableVisibleLength = redacted.startsWith(visible) ? visible.length : 0
    const safeLength = final ? redacted.length : Math.max(stableVisibleLength, guardedLength)
    return patchFor(redacted.slice(0, safeLength))
  }

  return {
    push(delta) {
      source += String(delta || '')
      return render(false)
    },
    flush() {
      return render(true)
    },
    text() {
      return visible
    },
  }
}

export function installSessionPersistenceRedaction(sessionManager) {
  if (!sessionManager || sessionManager[PERSISTENCE_REDACTION]) return sessionManager
  Object.defineProperty(sessionManager, PERSISTENCE_REDACTION, { value: true })

  const appendMessage = sessionManager.appendMessage?.bind(sessionManager)
  if (appendMessage) sessionManager.appendMessage = (message) => appendMessage(redactSecretValue(message))

  const appendCustomMessageEntry = sessionManager.appendCustomMessageEntry?.bind(sessionManager)
  if (appendCustomMessageEntry) {
    sessionManager.appendCustomMessageEntry = (customType, content, display, details) => appendCustomMessageEntry(
      customType,
      redactSecretValue(content),
      display,
      redactSecretValue(details),
    )
  }

  const appendCustomEntry = sessionManager.appendCustomEntry?.bind(sessionManager)
  if (appendCustomEntry) sessionManager.appendCustomEntry = (customType, data) => appendCustomEntry(customType, redactSecretValue(data))

  const appendCompaction = sessionManager.appendCompaction?.bind(sessionManager)
  if (appendCompaction) {
    sessionManager.appendCompaction = (summary, firstKeptEntryId, tokensBefore, details, fromHook) => appendCompaction(
      redactSecretText(summary),
      firstKeptEntryId,
      tokensBefore,
      redactSecretValue(details),
      fromHook,
    )
  }
  return sessionManager
}

function redactJsonLine(line) {
  if (!line.trim()) return line
  try {
    return JSON.stringify(redactSecretValue(JSON.parse(line)))
  } catch {
    return redactSecretText(line)
  }
}

export async function redactPersistedSessionFiles(sessionDir) {
  let changedFiles = 0
  let entries
  try {
    entries = await readdir(sessionDir, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return 0
    throw error
  }
  for (const entry of entries) {
    if (!entry.isFile() || !entry.name.toLowerCase().endsWith('.jsonl')) continue
    const path = join(sessionDir, entry.name)
    const source = await readFile(path, 'utf8')
    const trailingNewline = /\r?\n$/.test(source)
    const redacted = source.split(/\r?\n/).map(redactJsonLine).join('\n')
    const output = trailingNewline && !redacted.endsWith('\n') ? `${redacted}\n` : redacted
    if (output === source) continue
    const temporary = `${path}.redacting-${process.pid}`
    try {
      await writeFile(temporary, output, 'utf8')
      await rename(temporary, path)
      changedFiles += 1
    } finally {
      await unlink(temporary).catch(() => {})
    }
  }
  return changedFiles
}
