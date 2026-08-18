// 展示层格式化工具：相对时间 / token 数缩写 / 工作区名 / 文件大小。
// 相对时间在跨天后退化为日期，避免长会话列表里出现令人困惑的“3天前”
// 与具体日期混排；token 缩写随数量级调节精度（10万以上取整）。
// 相对时间：距现在 <1 分钟显示“刚刚”，<1 小时显示 N 分钟前，
// <1 天显示 N 小时前，跨天后退化为月/日日期——避免长会话列表里
// 出现“3天前”与具体日期混排的困惑；英文用 Intl.RelativeTimeFormat。
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

// token 数量缩写：千位用 K、百万位用 M，精度随数量级降低
// （10 万以上取整、10 万以下两位小数），并去掉多余的尾零。
export function formatTokenCount(value: unknown) {
  const tokens = Number(value) || 0
  if (tokens >= 1_000_000)
    return `${(tokens / 1_000_000).toFixed(tokens >= 10_000_000 ? 1 : 2).replace(/\.0+$/, '')}M`
  if (tokens >= 1_000)
    return `${(tokens / 1_000).toFixed(tokens >= 100_000 ? 0 : tokens >= 10_000 ? 1 : 2).replace(/\.0+$/, '')}K`
  return String(tokens)
}

// 从路径提取工作区名（取最后一段），去除尾部斜杠；空路径返回本地化占位。
export function workspaceName(value: unknown, locale = 'zh-CN') {
  const path = String(value || '').replace(/[\\/]+$/, '')
  return path.split(/[\\/]/).pop() || path || (locale === 'en-US' ? 'No folder set' : '未设置目录')
}

// 文件大小：B/KB/MB 自适应，<1KB 精确到字节，MB 保留一位小数。
export function formatFileSize(size: number | null | undefined) {
  if (!size) return ''
  if (size < 1024) return `${size} B`
  if (size < 1024 * 1024) return `${Math.round(size / 1024)} KB`
  return `${(size / 1024 / 1024).toFixed(1)} MB`
}
