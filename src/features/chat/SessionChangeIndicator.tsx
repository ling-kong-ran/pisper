import { useEffect, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useI18n } from '@/app/use-i18n'
import { getSessionChangeSummary } from './session-change-summary-api'

export function SessionChangeIndicator({
  sessionId,
  lastCompletedAt,
  streaming,
}: {
  sessionId: string
  lastCompletedAt?: string | null
  streaming?: boolean
}) {
  const { t } = useI18n()
  const anchorRef = useRef<HTMLSpanElement>(null)
  const [visible, setVisible] = useState(false)
  useEffect(() => {
    const node = anchorRef.current
    if (!node || visible) return
    if (!('IntersectionObserver' in window)) {
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
    queryKey: ['session-change-summary', sessionId, lastCompletedAt],
    queryFn: () => getSessionChangeSummary(sessionId),
    enabled: visible,
    staleTime: 60_000,
    retry: 0,
    refetchInterval: streaming && visible ? 20_000 : false,
  })
  if (data?.status !== 'known' || !data.changedFiles) {
    return <span ref={anchorRef} />
  }
  return (
    <span
      ref={anchorRef}
      className="ml-1 inline-flex items-center gap-1 tabular-nums before:mr-1 before:content-['·']"
    >
      {t('chat:chatHistoryPage.changeSummary', {
        count: data.changedFiles,
        added: data.added || 0,
        removed: data.removed || 0,
      })}
    </span>
  )
}
