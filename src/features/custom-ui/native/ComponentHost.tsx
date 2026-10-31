// 原生组件宿主（规格 5.6、计划 §4.3）：每个实例一份独立的 ErrorBoundary + Suspense，
// 按 instanceId@rev 重挂（热重载时属性与放置不变）。负责：模块加载与样式注入、ctx 生命周期、尺寸观察、
// 熔断（60 秒 3 次 → 已暂停卡）、慢提交警告、安全模式占位、错误上报。卡片头（名称、⋯ 菜单）由放置宿主绘制。
// 文案只用懒加载的 custom-ui 命名空间（由 placements.ts 的懒加载工厂先注册）。
import * as React from 'react'
import {
  Component as ReactComponent,
  Suspense,
  createElement,
  use,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ErrorInfo,
  type ReactNode,
} from 'react'
import type { ComponentSurface, OverviewCardSize } from '@shared/custom-ui-manifest.mjs'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import type { ComponentRow } from '@/features/custom-ui/custom-ui-api'
import { createCommitTimer, type CommitSample } from './commit-timer'
import { html } from './html'
import {
  componentCircuitBreaker,
  componentErrorReporter,
  componentLogBuffer,
  componentModuleLoader,
  componentSafeMode,
  componentStyleRegistry,
  createComponentServices,
} from './host-runtime'
import { createComponentLogger, errorDetails, type ComponentLogger } from './log'
import { ComponentLoadError } from './module-loader'
import { createComponentRuntime, type ComponentRuntime, type ComponentSessionInfo } from './sdk'
import { ComponentSessionFeed } from './sessions-source'
import { computeComponentSize, useElementBox, type ComponentSize } from './size'
import { useComponentTheme } from './theme'
import { uiKit } from './ui-kit'

export type ComponentHostInstance = {
  instanceId: string
  surface: ComponentSurface
  title?: string
  props: Record<string, unknown>
  size?: OverviewCardSize | null
}

export type ComponentHostProps = {
  component: ComponentRow
  instance: ComponentHostInstance
  // 工作室预览舞台：ctx.placement.preview 为 true，熔断与日志按 preview:<id> 单独计。
  preview?: boolean
  session?: ComponentSessionInfo
  notify(message: string): void
  onOpenSession?(id: string): void
  // [让代理修复]（S7 接线）：没有传时不显示这个按钮。
  onRequestFix?(summary: string): void
  className?: string
}

const KIT = Object.freeze({ React, html, ui: uiKit, useTheme: useComponentTheme })
const noop = () => {}

function instanceKey(component: ComponentRow, instance: ComponentHostInstance, preview: boolean) {
  return preview ? `preview:${component.id}` : instance.instanceId
}

export function ComponentHost({
  component,
  instance,
  preview = false,
  session,
  notify,
  onOpenSession,
  onRequestFix,
  className,
}: ComponentHostProps) {
  const { t, language } = useI18n()
  const key = instanceKey(component, instance, preview)
  const safeMode = componentSafeMode()
  const mode = useSyncExternalStore(safeMode.subscribe, safeMode.mode, safeMode.mode)
  const breaker = componentCircuitBreaker()
  useSyncExternalStore(breaker.subscribe, breaker.version, breaker.version)
  const paused = breaker.isPaused(key)
  const ref = useRef<HTMLDivElement>(null)
  const box = useElementBox(ref)
  const [attempt, setAttempt] = useState(0)
  const [slow, setSlow] = useState<number | null>(null)
  const manifest = component.manifest
  const rev = component.rev
  const runnable =
    component.kind === 'native' && component.status === 'ready' && Boolean(manifest && rev)
  const active = runnable && mode === 'normal' && !paused
  const logger = useMemo(
    () =>
      createComponentLogger({
        componentId: component.id,
        instanceId: key,
        surface: instance.surface,
        buffer: componentLogBuffer(),
        reporter: componentErrorReporter(),
      }),
    [component.id, instance.surface, key],
  )

  // 安全模式哨兵：首次挂载、加载模块之前同步写入；卸载时也算结束，避免没提交就离开的实例让哨兵残留。
  useLayoutEffect(() => {
    if (!active) return
    safeMode.beginLoad(key)
    return () => safeMode.endLoad(key)
  }, [active, key, safeMode])

  const size = computeComponentSize({
    surface: instance.surface,
    cardSize: instance.size,
    width: box.width,
    height: box.height,
  })

  let content: ReactNode = null
  if (!runnable) {
    content = <HostNotice title={t('custom-ui:host.unavailableTitle')} />
  } else if (mode !== 'normal') {
    content = (
      <HostNotice
        title={t('custom-ui:host.safeModeTitle')}
        description={t('custom-ui:host.safeModeHint')}
      />
    )
  } else if (paused) {
    content = (
      <HostNotice
        title={t('custom-ui:host.pausedTitle')}
        description={t('custom-ui:host.pausedHint')}
        actions={
          <Button
            size="xs"
            variant="secondary"
            onClick={() => {
              setSlow(null)
              breaker.resume(key)
              setAttempt((value) => value + 1)
            }}
          >
            {t('custom-ui:host.resume')}
          </Button>
        }
      />
    )
  } else if (box.measured && manifest && rev) {
    content = (
      <InstanceBoundary
        key={`${instance.instanceId}@${rev}#${attempt}`}
        onCrash={(error) => {
          logger.record(error instanceof ComponentLoadError ? 'load' : 'render', 'error', error)
          safeMode.endLoad(key)
          breaker.record(key)
        }}
        fallback={(error) => (
          <ErrorCard
            error={error}
            onReload={() => setAttempt((value) => value + 1)}
            onRequestFix={onRequestFix}
          />
        )}
      >
        <Suspense fallback={<Skeleton className="size-full rounded-none" />}>
          {manifest.capabilities['sessions.read'] && <ComponentSessionFeed />}
          <LoadedComponent
            component={component}
            instance={instance}
            preview={preview}
            session={session}
            size={size}
            language={language}
            logger={logger}
            notify={notify}
            onOpenSession={onOpenSession ?? noop}
            onCallbackError={() => breaker.record(key)}
            onCommit={(sample, first) => {
              if (first) safeMode.endLoad(key)
              // 只在警告第一次出现时记录并显示；之后每次提交都写状态会形成重渲染循环。
              if (sample?.warning && slow === null) {
                const duration = Math.round(sample.duration)
                logger.record('slow', 'warn', `slow commit ${duration}ms`)
                setSlow(duration)
              }
            }}
          />
        </Suspense>
      </InstanceBoundary>
    )
  }

  return (
    <div
      ref={ref}
      data-component-host={component.id}
      data-component-instance={instance.instanceId}
      data-component-state={
        !runnable ? 'unavailable' : mode !== 'normal' ? 'safe-mode' : paused ? 'paused' : 'active'
      }
      className={cn('relative size-full min-h-0 min-w-0 overflow-hidden', className)}
    >
      {content}
      {active && slow !== null && (
        <div
          role="status"
          className="absolute top-1 right-1 flex items-center gap-1 rounded-sm border border-stroke-strong bg-panel px-1.5 py-0.5 text-xs text-content-secondary shadow-key"
        >
          <span>{t('custom-ui:host.slowWarning', { ms: slow })}</span>
          <Button size="xs" variant="ghost" onClick={() => breaker.pause(key)}>
            {t('custom-ui:host.pause')}
          </Button>
        </div>
      )}
    </div>
  )
}

function LoadedComponent({
  component,
  instance,
  preview,
  session,
  size,
  language,
  logger,
  notify,
  onOpenSession,
  onCallbackError,
  onCommit,
}: {
  component: ComponentRow
  instance: ComponentHostInstance
  preview: boolean
  session?: ComponentSessionInfo
  size: ComponentSize
  language: 'zh-CN' | 'en-US'
  logger: ComponentLogger
  notify(message: string): void
  onOpenSession(id: string): void
  onCallbackError(error: unknown): void
  onCommit(sample: CommitSample | null, first: boolean): void
}) {
  // 调用方保证 manifest 与 rev 存在（runnable 才会渲染这里）。
  const manifest = component.manifest!
  const rev = component.rev!
  const Component = use(componentModuleLoader().load(component.id, rev, manifest.entry))
  // 回调里读最新的宿主函数，ctx 不必因为父组件重渲染而重建。
  const latest = useRef({ notify, onOpenSession, onCallbackError, language })
  latest.current = { notify, onOpenSession, onCallbackError, language }
  // 工厂放进 ref：effect 只在 runtime 换掉时重跑，工厂本身每次渲染都是新的也没关系。
  const createRef = useRef(() =>
    createComponentRuntime({
      manifest,
      getLanguage: () => latest.current.language,
      kit: KIT,
      services: createComponentServices(component.id, {
        notify: (text) => latest.current.notify(text),
        openSession: (id) => latest.current.onOpenSession(id),
      }),
      logger,
      onCallbackError: (error) => latest.current.onCallbackError(error),
    }),
  )
  const [runtime, setRuntime] = useState<ComponentRuntime>(() => createRef.current())
  // StrictMode（开发构建）会模拟一次卸载再挂载：延后一拍释放，重挂时取消，组件拿到的 ctx 始终是活的。
  const disposeTimer = useRef<number | null>(null)
  useEffect(() => {
    if (disposeTimer.current !== null) {
      window.clearTimeout(disposeTimer.current)
      disposeTimer.current = null
    }
    if (runtime.disposed) {
      setRuntime(createRef.current())
      return
    }
    return () => {
      disposeTimer.current = window.setTimeout(() => {
        disposeTimer.current = null
        runtime.dispose()
      }, 0)
    }
  }, [runtime])
  useEffect(() => {
    const styles = manifest.styles
    if (!styles.length) return
    return componentStyleRegistry().retain(component.id, rev, styles)
  }, [component.id, manifest.styles, rev])

  const timer = useRef(createCommitTimer())
  const committed = useRef(false)
  timer.current.start(performance.now())
  const placement = {
    surface: instance.surface,
    instanceId: instance.instanceId,
    title: instance.title ?? '',
    preview,
  }
  const ctx = runtime.context({ size, placement, session })
  return (
    <>
      {createElement(Component as unknown as React.FunctionComponent<Record<string, unknown>>, {
        props: instance.props,
        size: ctx.size,
        placement: ctx.placement,
        ctx,
      })}
      <CommitSentinel
        onCommit={() => {
          const sample = timer.current.commit(performance.now())
          const first = !committed.current
          committed.current = true
          onCommit(sample, first)
        }}
      />
    </>
  )
}

// 子树末尾的哨兵：它的 layout effect 在组件子树的 layout effect 之后运行，用来量本次提交耗时。
function CommitSentinel({ onCommit }: { onCommit(): void }) {
  useLayoutEffect(() => {
    onCommit()
  })
  return null
}

type BoundaryProps = {
  children: ReactNode
  onCrash(error: unknown): void
  fallback(error: unknown): ReactNode
}

class InstanceBoundary extends ReactComponent<BoundaryProps, { error: unknown; failed: boolean }> {
  state = { error: null as unknown, failed: false }

  static getDerivedStateFromError(error: unknown) {
    return { error, failed: true }
  }

  componentDidCatch(error: unknown, _info: ErrorInfo) {
    this.props.onCrash(error)
  }

  render() {
    return this.state.failed ? this.props.fallback(this.state.error) : this.props.children
  }
}

function ErrorCard({
  error,
  onReload,
  onRequestFix,
}: {
  error: unknown
  onReload(): void
  onRequestFix?(summary: string): void
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const details = errorDetails(error)
  const load = error instanceof ComponentLoadError
  return (
    <HostNotice
      tone="error"
      title={load ? t('custom-ui:host.loadErrorTitle') : t('custom-ui:host.renderErrorTitle')}
      actions={
        <>
          <Button size="xs" variant="ghost" aria-expanded={open} onClick={() => setOpen(!open)}>
            {open ? t('custom-ui:host.hideError') : t('custom-ui:host.viewError')}
          </Button>
          <Button size="xs" variant="secondary" onClick={onReload}>
            {t('custom-ui:host.reload')}
          </Button>
          {onRequestFix && (
            <Button
              size="xs"
              variant="ghost"
              onClick={() => onRequestFix(`${load ? 'load' : 'render'}: ${details.message}`)}
            >
              {t('custom-ui:host.askAgentFix')}
            </Button>
          )}
        </>
      }
    >
      {open && (
        <pre className="max-h-32 w-full overflow-auto rounded-sm bg-surface-subtle p-2 text-left font-mono text-xs whitespace-pre-wrap text-content-secondary">
          {details.stack || details.message}
        </pre>
      )}
    </HostNotice>
  )
}

function HostNotice({
  title,
  description,
  actions,
  children,
  tone = 'neutral',
}: {
  title: ReactNode
  description?: ReactNode
  actions?: ReactNode
  children?: ReactNode
  tone?: 'neutral' | 'error'
}) {
  return (
    <div
      role={tone === 'error' ? 'alert' : 'status'}
      className="flex size-full min-h-0 flex-col items-start justify-center gap-1.5 overflow-auto p-3 text-left"
    >
      <p className={cn('text-sm font-medium', tone === 'error' ? 'text-st-fail' : 'text-content')}>
        {title}
      </p>
      {description && <p className="text-xs text-content-muted">{description}</p>}
      {children}
      {actions && <div className="flex flex-wrap items-center gap-1">{actions}</div>}
    </div>
  )
}
