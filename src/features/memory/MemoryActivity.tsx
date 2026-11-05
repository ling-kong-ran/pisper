// 整理记录：记忆每次被新增、替换、合并、归档都写在这里，附理由；能撤销的给撤销键。
// 记忆不需要确认，靠这份记录保证事后看得到、改得回。连到没有这个接口的旧 Runtime 时不算错误。
import { useCallback, useEffect, useRef, useState } from 'react'
import { Undo2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { FeatureAlert, FeatureList, FeatureRow } from '@/components/app/FeaturePage'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { ApiError } from '@/lib/http'
import { fetchMemoryEvents, undoMemoryEvent } from './memory-api'
import {
  formatMemoryTime,
  memoryActorLabel,
  memoryEventPresentation,
  memoryReasonLabel,
} from './memory-labels'
import type { MemoryEvent } from './memory-types'

const PAGE_SIZE = 50

export function MemoryActivity({
  spaceId,
  refreshKey,
  onChanged,
}: {
  // 'all' 表示全部空间。
  spaceId: string
  refreshKey: number
  onChanged: (message: string) => Promise<void> | void
}) {
  const { t, language } = useI18n()
  const [events, setEvents] = useState<MemoryEvent[]>([])
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [hasMore, setHasMore] = useState(false)
  const [unsupported, setUnsupported] = useState(false)
  const [error, setError] = useState('')
  const [undoing, setUndoing] = useState('')
  const revision = useRef(0)
  const space = spaceId === 'all' ? '' : spaceId

  const load = useCallback(async () => {
    const current = ++revision.current
    setLoading(true)
    setError('')
    try {
      const result = await fetchMemoryEvents({ spaceId: space, limit: PAGE_SIZE })
      if (current !== revision.current) return
      setEvents(result.events)
      setHasMore(result.events.length >= PAGE_SIZE)
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
    const before = events.at(-1)?.at
    if (!before || loadingMore) return
    const current = revision.current
    setLoadingMore(true)
    try {
      const result = await fetchMemoryEvents({ spaceId: space, before, limit: PAGE_SIZE })
      if (current !== revision.current) return
      setEvents((previous) => {
        const known = new Set(previous.map((event) => event.id))
        return [...previous, ...result.events.filter((event) => !known.has(event.id))]
      })
      setHasMore(result.events.length >= PAGE_SIZE)
    } catch (loadError) {
      if (current === revision.current)
        setError(loadError instanceof Error ? loadError.message : String(loadError))
    } finally {
      setLoadingMore(false)
    }
  }

  const undo = async (event: MemoryEvent) => {
    if (undoing) return
    setUndoing(event.id)
    setError('')
    try {
      await undoMemoryEvent(event.id)
      await onChanged(t('memory:memoryPage.eventUndone'))
    } catch (undoError) {
      setError(undoError instanceof Error ? undoError.message : String(undoError))
    } finally {
      setUndoing('')
    }
  }

  if (unsupported)
    return <InlineEmptyState compact title={t('memory:memoryPage.activityUnsupported')} />
  if (loading && !events.length)
    return <PageState kind="loading" title={t('common:pageTemplate.loading')} />
  if (error && !events.length)
    return (
      <PageState
        kind="error"
        title={t('memory:memoryPage.activityLoadFailed')}
        description={error}
        action={
          <Button variant="secondary" className="max-[650px]:h-11" onClick={() => void load()}>
            {t('common:pageTemplate.retry')}
          </Button>
        }
      />
    )
  if (!events.length) return <InlineEmptyState compact title={t('memory:memoryPage.noActivity')} />
  return (
    <div className="flex min-w-0 flex-col gap-3">
      {error && <FeatureAlert className="mt-0">{error}</FeatureAlert>}
      <FeatureList>
        {events.map((event) => {
          const { icon: Icon, label } = memoryEventPresentation(event.action, t)
          const reason = event.reason ? memoryReasonLabel(event.reason, t) : ''
          return (
            <FeatureRow
              key={event.id}
              data-memory-event={event.action}
              icon={<Icon />}
              name={label}
              // 替换前后常常同名，同一个标题只写一次。
              description={[...new Set(event.titles || [])].join('、') || undefined}
              meta={
                <span className="truncate tabular-nums">
                  {formatMemoryTime(event.at, language)} · {memoryActorLabel(event.actor, t)}
                  {reason ? ` · ${reason}` : ''}
                </span>
              }
              status={event.undoneAt ? t('memory:memoryPage.undone') : undefined}
              actions={
                event.undoable ? (
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={Boolean(undoing)}
                    aria-label={`${t('memory:memoryPage.undo')}: ${label}`}
                    onClick={() => void undo(event)}
                  >
                    <Undo2 />
                    {t('memory:memoryPage.undo')}
                  </Button>
                ) : undefined
              }
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
