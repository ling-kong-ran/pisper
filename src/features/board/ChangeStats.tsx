// 已完成代理的改动统计“6 个文件 +214 −97”：增删用 diff 的红绿（规格 6.1 例外），文件数用次要色。
// card 自带统计（Runtime 在写入类工具结束时缓存）时直接显示；没有 card 的旧 Runtime 只为进入可视区的行
// 懒读 /change-summary（与历史页的指示器共用同一份缓存），读不到或没有改动就什么也不显示，不造数字。
import { useEffect, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useI18n } from '@/app/use-i18n'
import { getSessionChangeSummary } from '@/features/chat/public-launch'

export function CardChangeStats({
  files,
  added,
  removed,
}: {
  files: number
  added: number
  removed: number
}) {
  const { t } = useI18n()
  return (
    <span
      data-change-stats
      className="flex min-w-0 items-center gap-1.5 text-xs whitespace-nowrap text-content-secondary"
    >
      <span>{t('board:card.files', { count: files })}</span>
      <span className="font-mono text-success tabular-nums">+{added}</span>
      <span className="font-mono text-danger tabular-nums">−{removed}</span>
    </span>
  )
}

export function ChangeStats({
  sessionId,
  completedAt,
}: {
  sessionId: string
  completedAt: string | null
}) {
  const anchor = useRef<HTMLSpanElement>(null)
  const [visible, setVisible] = useState(false)
  useEffect(() => {
    const node = anchor.current
    if (!node || visible) return
    if (typeof IntersectionObserver === 'undefined') {
      setVisible(true)
      return
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setVisible(true)
          observer.disconnect()
        }
      },
      { rootMargin: '120px' },
    )
    observer.observe(node)
    return () => observer.disconnect()
  }, [visible])
  const { data } = useQuery({
    queryKey: ['session-change-summary', sessionId, completedAt],
    queryFn: () => getSessionChangeSummary(sessionId),
    enabled: visible,
    staleTime: 60_000,
    retry: 0,
  })
  if (data?.status !== 'known' || !data.changedFiles) return <span ref={anchor} />
  return (
    <span ref={anchor} className="contents">
      <CardChangeStats
        files={data.changedFiles}
        added={data.added || 0}
        removed={data.removed || 0}
      />
    </span>
  )
}
