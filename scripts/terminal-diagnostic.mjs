// 终端合成页诊断共用文本脱敏，避免控制台和结构化现场采用不同规则。
export function createTerminalDiagnosticSanitizer(roots = {}) {
  const slash = /[\\/]/.source
  const boundary = /(?=$|[\\/\s"':),<>\]}])/.source
  // 更长的 owned 路径先替换，保留 workspace 内的相对位置；不访问路径内容。
  const replacements = [
    [roots.workspace, '<workspace>'],
    [roots.outputRoot, '<owned-output>'],
    [roots.repositoryRoot, '<repository>'],
    [roots.homeRoot, '<home>'],
  ]
    .filter(([path]) => typeof path === 'string' && path.length > 1)
    .map(([path, label]) => ({
      path: path
        .replaceAll('\\', '/')
        .replace(/^\/\/\?\/UNC\//i, '//')
        .replace(/^\/\/\?\//, '')
        .replace(/\/+$/, ''),
      label,
    }))
    .filter(({ path }) => path.length > 1)
    .sort((left, right) => right.path.length - left.path.length)
    .flatMap(({ path, label }) => {
      const unc = path.startsWith('//')
      const windows = unc || /^[a-z]:\//i.test(path)
      return [...new Set([path, encodeURI(path)])].map((spelling) => {
        const body = (unc ? spelling.slice(2) : spelling)
          .split('/')
          .map((part) => part.replace(/[.*+?^{}$()|[\]\\]/g, '\\$&'))
          .join(windows ? /[\\/]+/.source : slash)
        const prefix = unc
          ? /[\\/]{2,}(?:\?[\\/]+UNC[\\/]+)?/.source
          : windows
            ? /(?:[\\/]{2,}\?[\\/]+)?/.source
            : ''
        return { pattern: new RegExp(prefix + body + boundary, windows ? 'gi' : 'g'), label }
      })
    })
  const text = (value) =>
    replacements
      .reduce((current, { pattern, label }) => current.replaceAll(pattern, label), value)
      .replaceAll('rust_usability_sandbox_only', '[REDACTED]')
      .replaceAll(/synthetic-[\w-]*(?:key|secret)/g, '[REDACTED]')
      .replaceAll(
        /([?&](?:token|api_key|apiKey|key|secret|password)=)[^&\s"'\\<>]+/gi,
        '$1[REDACTED]',
      )
      .replaceAll(/(Bearer\s+)[^\s"'\\<>]+/gi, '$1[REDACTED]')
      .replaceAll(/(__pisper_desktop=)[^;\s"'\\<>]+/g, '$1[REDACTED]')
  const sanitize = (value) => {
    if (typeof value === 'string') return text(value)
    if (Array.isArray(value)) return value.map(sanitize)
    if (value && typeof value === 'object')
      return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, sanitize(entry)]))
    return value
  }
  return sanitize
}
