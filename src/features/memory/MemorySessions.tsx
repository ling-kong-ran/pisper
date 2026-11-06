// 会话摘要：一次对话做了什么、定了什么、还剩什么。对话停下约 30 分钟后自动写，不进年轮也不进自动回忆，
// 只有这里和代理的记忆工具看得到。连到没有这个接口的旧 Runtime 时不算错误。
import { useCallback, useEffect, useRef, useState } from 'react'
import { MessageSquareText } from 'lucide-react'
import { useNavigate } from 'react-router-dom'
import { sessionPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { FeatureAlert, FeatureList, FeatureRow } from '@/components/app/FeaturePage'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { ApiError } from '@/lib/http'
import { fetchMemoryDetail, fetchMemorySessionDigests } from './memory-api'
import { formatMemoryTime, memorySpaceLabel } from './memory-labels'
import type { MemoryDetail, MemorySessionDigest, MemorySpace } from './memory-types'

const PAGE_SIZE = 30

function SessionDigestDetail({ digest }: { digest: MemorySessionDigest }) {
  const { t } = useI18n()
  const navigate = useNavigate()
  // 事实只是补充：读不到时摘要照常显示。
  const [facts, setFacts] = useState<NonNullable<MemoryDetail['children']>>([])

  useEffect(() => {
    const controller = new AbortController()
    fetchMemoryDetail(digest.id, controller.signal).then(
      (detail) => setFacts(detail.children || []),
      () => setFacts([]),
    )
    return () => controller.abort()
  }, [digest.id])

  return (
    <div className="flex min-w-0 flex-col gap-3">
      <p className="m-0 text-sm leading-relaxed break-words whitespace-pre-wrap">{digest.text}</p>
      {facts.length > 0 && (
        <div className="min-w-0">
          <p className="m-0 text-xs font-medium text-content-muted">
            {t('memory:memoryPage.sessionFacts')}
          </p>
          <ul className="m-0 mt-1 list-disc pl-4 text-sm leading-relaxed">
            {facts.map((fact) => (
              <li key={fact.id} className="break-words">
                {fact.title}
              </li>
            ))}
          </ul>
        </div>
      )}
      {digest.sessionAvailable && (
        <div>
          <Button
            variant="secondary"
            size="sm"
            className="max-[650px]:h-11"
            onClick={() => navigate(sessionPath(digest.sessionId))}
          >
            {t('memory:memoryPage.openSession')}
          </Button>
        </div>
      )}
    </div>
  )
}

export function MemorySessions({
  spaceId,
  spaces,
  refreshKey,
}: {
  // 'all' 表示全部空间。
  spaceId: string
  spaces: MemorySpace[]
  refreshKey: number
}) {
  const { t, language } = useI18n()
  const [digests, setDigests] = useState<MemorySessionDigest[]>([])
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [hasMore, setHasMore] = useState(false)
  const [unsupported, setUnsupported] = useState(false)
  const [error, setError] = useState('')
  const [expandedId, setExpandedId] = useState('')
  const revision = useRef(0)
  const space = spaceId === 'all' ? '' : spaceId

  const load = useCallback(async () => {
    const current = ++revision.current
    setLoading(true)
    setError('')
    try {
      const result = await fetchMemorySessionDigests({ spaceId: space, limit: PAGE_SIZE })
      if (current !== revision.current) return
      setDigests(result.sessions)
      setHasMore(result.sessions.length >= PAGE_SIZE)
      setUnsupported(false)
    } catch (loadError) {
      if (current !== revision.current) return
      if (loadError instanceof ApiError && loadError.status === 404) setUnsupported(true)
      else setError(loadError instanceof Error ? loadError.message : String(loadError))
    } finally {
      if (current === revision.current) setLoading(false)
    }
  }, [space])

  useEffect(() => {
    void load()
    return () => {
      revision.current += 1
    }
  }, [load, refreshKey])

  const loadMore = async () => {
    const before = digests.at(-1)?.windowEnd
    if (!before || loadingMore) return
    const current = revision.current
    setLoadingMore(true)
    try {
      const result = await fetchMemorySessionDigests({ spaceId: space, before, limit: PAGE_SIZE })
      if (current !== revision.current) return
      setDigests((previous) => {
        const known = new Set(previous.map((digest) => digest.id))
        return [...previous, ...result.sessions.filter((digest) => !known.has(digest.id))]
      })
      setHasMore(result.sessions.length >= PAGE_SIZE)
    } catch (loadError) {
      if (current === revision.current)
        setError(loadError instanceof Error ? loadError.message : String(loadError))
    } finally {
      setLoadingMore(false)
    }
  }

  if (unsupported)
    return <InlineEmptyState compact title={t('memory:memoryPage.sessionsUnsupported')} />
  if (loading && !digests.length)
    return <PageState kind="loading" title={t('common:pageTemplate.loading')} />
  if (error && !digests.length)
    return (
      <PageState
        kind="error"
        title={t('memory:memoryPage.sessionsLoadFailed')}
        description={error}
        action={
          <Button variant="secondary" className="max-[650px]:h-11" onClick={() => void load()}>
            {t('common:pageTemplate.retry')}
          </Button>
        }
      />
    )
  if (!digests.length)
    return <InlineEmptyState compact title={t('memory:memoryPage.noSessionDigests')} />
  return (
    <div className="flex min-w-0 flex-col gap-3">
      {error && <FeatureAlert className="mt-0">{error}</FeatureAlert>}
      <FeatureList>
        {digests.map((digest) => {
          const name = digest.sessionName || t('memory:memoryPage.sessionUntitled')
          const digestSpace = spaces.find((item) => item.id === digest.spaceId)
          return (
            <FeatureRow
              key={digest.id}
              data-memory-session={digest.sessionId}
              icon={<MessageSquareText />}
              name={name}
              // 展开后详情里有全文，行上不再重复。
              description={expandedId === digest.id ? undefined : digest.text}
              meta={
                <span className="truncate tabular-nums">
                  {formatMemoryTime(digest.windowEnd, language)}
                  {spaceId === 'all' && digestSpace ? ` · ${memorySpaceLabel(digestSpace, t)}` : ''}
                </span>
              }
              expanded={expandedId === digest.id}
              onExpandedChange={(open) => setExpandedId(open ? digest.id : '')}
              toggleLabel={name}
              detail={<SessionDigestDetail digest={digest} />}
            />
          )
        })}
      </FeatureList>
      {hasMore && (
        <div className="flex justify-center">
          <Button
            variant="secondary"
            size="sm"
            className="max-[650px]:h-11"
            loading={loadingMore}
            disabled={loadingMore}
            onClick={() => void loadMore()}
          >
            {t('memory:memoryPage.loadMore')}
          </Button>
        </div>
      )}
    </div>
  )
}
