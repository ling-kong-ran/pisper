export function relativeTime(value: string | number | Date | null | undefined, locale = 'zh-CN') {
  const english = locale === 'en-US'
  if (!value) return english ? 'Just now' : '刚刚'
  const distance = Date.now() - new Date(value).getTime()
  if (distance < 60_000) return english ? 'Just now' : '刚刚'
  if (distance < 3_600_000) {
    const minutes = Math.floor(distance / 60_000)
    return english
      ? new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }).format(-minutes, 'minute')
      : `${minutes} 分钟前`
  }
  if (distance < 86_400_000) {
    const hours = Math.floor(distance / 3_600_000)
    return english
      ? new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }).format(-hours, 'hour')
      : `${hours} 小时前`
  }
  return new Date(value).toLocaleDateString(locale, { month: 'numeric', day: 'numeric' })
}

export function formatTokenCount(value: unknown) {
  const tokens = Number(value) || 0
  if (tokens >= 1_000_000)
    return `${(tokens / 1_000_000).toFixed(tokens >= 10_000_000 ? 1 : 2).replace(/\.0+$/, '')}M`
  if (tokens >= 1_000)
    return `${(tokens / 1_000).toFixed(tokens >= 100_000 ? 0 : tokens >= 10_000 ? 1 : 2).replace(/\.0+$/, '')}K`
  return String(tokens)
}

export function workspaceName(value: unknown, locale = 'zh-CN') {
  const path = String(value || '').replace(/[\\/]+$/, '')
  return path.split(/[\\/]/).pop() || path || (locale === 'en-US' ? 'No folder set' : '未设置目录')
}

export function formatFileSize(size: number | null | undefined) {
  if (!size) return ''
  if (size < 1024) return `${size} B`
  if (size < 1024 * 1024) return `${Math.round(size / 1024)} KB`
  return `${(size / 1024 / 1024).toFixed(1)} MB`
}
