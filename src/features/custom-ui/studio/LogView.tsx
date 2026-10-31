// 日志页（规格 5.6，计划 §4.4）：本页日志（ctx.log 与宿主捕获的错误，含 info，只在内存里）
// 与 Runtime 记录的错误（GET …/errors，所有放置与设备上报的 warn / error，重启后清空）。
// 两者来源不同、可能重复（本页的 warn / error 也会上报），分两段显示，不硬合并。
import { useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { ComponentErrorsData } from '@/features/custom-ui/custom-ui-api'
import { componentLogBuffer } from '@/features/custom-ui/native/host-runtime'
import type { ComponentLogEntry } from '@/features/custom-ui/native/log'

// 缓冲区的 read() 每次返回新数组：订阅变化后再读一次放进状态（不能直接当 useSyncExternalStore 的快照）。
function useLocalLog(componentId: string): ComponentLogEntry[] {
  const buffer = componentLogBuffer()
  const [entries, setEntries] = useState(() => buffer.read(componentId))
  useEffect(() => {
    setEntries(buffer.read(componentId))
    return buffer.subscribe((id) => {
      if (id === componentId) setEntries(buffer.read(componentId))
    })
  }, [buffer, componentId])
  return entries
}

function time(value: number | string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '' : date.toLocaleTimeString(undefined, { hour12: false })
}

function LogLine({
  at,
  level,
  kind,
  message,
  stack,
}: {
  at: number | string
  level: string
  kind: string
  message: string
  stack?: string
}) {
  return (
    <li className="flex min-w-0 flex-col gap-0.5 border-b border-stroke px-3 py-2 last:border-b-0">
      <span className="flex min-w-0 items-baseline gap-2 font-mono text-xs">
        <span className="flex-none text-content-muted tabular-nums">{time(at)}</span>
        <span
          className={cn(
            'flex-none uppercase',
            level === 'error' ? 'text-st-fail font-medium' : 'text-content-muted',
          )}
        >
          {level}
        </span>
        <span className="flex-none text-content-muted">{kind}</span>
        <span className="min-w-0 break-all text-content">{message}</span>
      </span>
      {stack && (
        <pre className="max-h-28 overflow-auto font-mono text-xs whitespace-pre-wrap text-content-muted">
          {stack}
        </pre>
      )}
    </li>
  )
}

export function LogView({
  componentId,
  errors,
  errorsFailed,
}: {
  componentId: string
  errors: ComponentErrorsData | undefined
  errorsFailed: boolean
}) {
  const { t } = useI18n()
  const buffer = componentLogBuffer()
  const local = useLocalLog(componentId)
  const remote = errors?.entries ?? []
  return (
    <div data-studio-logs className="flex min-h-0 min-w-0 flex-1 flex-col gap-3 overflow-y-auto">
      <section className="flex flex-col gap-1.5">
        <header className="flex items-center justify-between gap-2">
          <h3 className="text-xs font-medium text-content-secondary">
            {t('custom-ui:studio.logsLocal')}
          </h3>
          {local.length > 0 && (
            <Button size="xs" variant="ghost" onClick={() => buffer.clear(componentId)}>
              {t('custom-ui:studio.logsClear')}
            </Button>
          )}
        </header>
        {local.length ? (
          <ul className="overflow-hidden rounded-[var(--r-md)] border border-stroke bg-panel">
            {[...local].reverse().map((entry, index) => (
              <LogLine key={`${entry.at}:${index}`} {...entry} />
            ))}
          </ul>
        ) : (
          <p className="text-xs text-content-muted">{t('custom-ui:studio.logsLocalEmpty')}</p>
        )}
      </section>
      <section className="flex flex-col gap-1.5">
        <h3 className="text-xs font-medium text-content-secondary">
          {t('custom-ui:studio.logsRuntime')}
          {errors && errors.dropped > 0 && (
            <span className="ml-2 font-normal text-content-muted">
              {t('custom-ui:studio.logsDropped', { count: errors.dropped })}
            </span>
          )}
        </h3>
        {errorsFailed ? (
          <p className="text-xs text-content-muted">{t('custom-ui:studio.logsRuntimeFailed')}</p>
        ) : remote.length ? (
          <ul
            data-studio-runtime-logs
            className="overflow-hidden rounded-[var(--r-md)] border border-stroke bg-panel"
          >
            {[...remote].reverse().map((entry, index) => (
              <LogLine key={`${entry.at}:${index}`} {...entry} />
            ))}
          </ul>
        ) : (
          <p className="text-xs text-content-muted">{t('custom-ui:studio.logsRuntimeEmpty')}</p>
        )}
      </section>
    </div>
  )
}
