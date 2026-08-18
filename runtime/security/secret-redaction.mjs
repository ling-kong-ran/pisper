// 密钥脱敏：在错误信息/日志/模型输入中把疑似密钥（API Key/令牌/密码等）替换为占位符，
// 防止密钥经提示词或错误消息外泄。
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

const EXPLICIT_SENSITIVE_KEY_PATTERN =
  '(?:api[_ -]?key|access[_ -]?token|refresh[_ -]?token|auth[_ -]?token|client[_ -]?secret|app[_ -]?secret|password|passwd|authorization|cookie|credentials?)'
const GENERIC_TOKEN_KEY_PATTERN = '(?<![a-z0-9_])token(?![a-z0-9_])'
const QUOTED_SECRET = new RegExp(
  `((?:["'])?${EXPLICIT_SENSITIVE_KEY_PATTERN}(?:["'])?\\s*[:=]\\s*)(["'])([^\\r\\n]*?)\\2`,
  'gi',
)
const PLAIN_SECRET = new RegExp(
  `((?:^|[\\s,{;，；（(])(?:${EXPLICIT_SENSITIVE_KEY_PATTERN})\\s*[:=：]\\s*)(?!["']|\\[REDACTED SECRET\\])[^\\s,，;}；\\]]+`,
  'gim',
)
const CLI_SECRET = new RegExp(
  `(--?(?:${EXPLICIT_SENSITIVE_KEY_PATTERN})(?:=|\\s+))(?!\\[REDACTED SECRET\\])([^\\s"']+)`,
  'gi',
)
const QUOTED_GENERIC_TOKEN = new RegExp(
  `((?:["'])?${GENERIC_TOKEN_KEY_PATTERN}(?:["'])?\\s*[:=]\\s*)(["'])([^\\r\\n]*?)\\2`,
  'gi',
)
const PLAIN_GENERIC_TOKEN = new RegExp(
  `((?:^|[\\s,{;，；（(])${GENERIC_TOKEN_KEY_PATTERN}\\s*[:=：]\\s*)(?!["']|\\[REDACTED SECRET\\])([^\\s,，;}；\\]]+)`,
  'gim',
)
const CLI_GENERIC_TOKEN = new RegExp(
  `(--?token(?:=|\\s+))(?!\\[REDACTED SECRET\\])([^\\s"']+)`,
  'gi',
)

function normalizedKey(value) {
  return String(value || '')
    .toLowerCase()
    .replace(/[^a-z0-9]/g, '')
}

function looksLikeSecret(value) {
  if (typeof value !== 'string') return false
  const text = value.trim()
  if (!text || text === REDACTED_SECRET) return false
  if (/^Bearer\s+/i.test(text)) return true
  if (/^eyJ[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}$/i.test(text)) return true
  if (/^(?:sk|rk|pk|pcl|ghp|github_pat|xox[baprs])[-_][a-z0-9_-]{12,}$/i.test(text)) return true
  return (
    text.length >= 20 && !/\s/.test(text) && /[a-z]/i.test(text) && /(?:\d|[^a-z0-9])/i.test(text)
  )
}

function sensitiveKey(value, content) {
  const key = normalizedKey(value)
  if (key === 'token') return looksLikeSecret(content)
  return (
    ALWAYS_SENSITIVE_KEYS.has(key) ||
    /(?:apikey|secret|password|passwd|authorization|credential|accesstoken|refreshtoken|authtoken)$/.test(
      key,
    )
  )
}

export function redactSecretText(value) {
  return String(value ?? '')
    .replace(
      /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/g,
      REDACTED_SECRET,
    )
    .replace(/\b(Bearer\s+)[a-z0-9._~+/-]{8,}/gi, `$1${REDACTED_SECRET}`)
    .replace(
      /\b(?:postgres(?:ql)?|mysql|mariadb|mongodb(?:\+srv)?|redis):\/\/[^\s]+/gi,
      REDACTED_SECRET,
    )
    .replace(
      QUOTED_SECRET,
      (_match, prefix, quote) => `${prefix}${quote}${REDACTED_SECRET}${quote}`,
    )
    .replace(PLAIN_SECRET, (_match, prefix) => `${prefix}${REDACTED_SECRET}`)
    .replace(CLI_SECRET, (_match, prefix) => `${prefix}${REDACTED_SECRET}`)
    .replace(QUOTED_GENERIC_TOKEN, (match, prefix, quote, secret) =>
      looksLikeSecret(secret) ? `${prefix}${quote}${REDACTED_SECRET}${quote}` : match,
    )
    .replace(PLAIN_GENERIC_TOKEN, (match, prefix, secret) =>
      looksLikeSecret(secret) ? `${prefix}${REDACTED_SECRET}` : match,
    )
    .replace(CLI_GENERIC_TOKEN, (match, prefix, secret) =>
      looksLikeSecret(secret) ? `${prefix}${REDACTED_SECRET}` : match,
    )
    .replace(
      /([?&](?:(?:access|refresh|auth)[_-]?)?(?:token|key|secret|password|auth|credential)[^=&#\s]*=)(?!\[REDACTED SECRET\])[^&#\s]*/gi,
      `$1${REDACTED_SECRET}`,
    )
    .replace(/\beyJ[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}\b/gi, REDACTED_SECRET)
    .replace(/\b(?:sk|rk|pk|pcl|ghp|github_pat|xox[baprs])[-_][a-z0-9_-]{12,}\b/gi, REDACTED_SECRET)
}

export function containsSecretText(value) {
  const text = String(value ?? '')
  return redactSecretText(text) !== text
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
