// 预览栏 + 点阵舞台 + 状态栏（规格 5.1 工作台）：放置档位（总览 S / M、会话面板、功能页、浮窗）、
// 窗口（标准 / 最小 1080）、主题只有“跟随应用”（暗色 token 只定义在 :root，局部容器切换不了）。
// 舞台以 1:1 渲染真实组件（ComponentHost preview 模式），总览档位标出尺寸。
// 状态栏：错误数 · 上次渲染耗时 · 网络次数与主机 · 保存即热重载；渲染耗时走外部小仓库，
// 只让状态栏自己重渲染（预览宿主每次提交都上报，若写进父组件状态会形成重渲染循环）。
import { Suspense, useMemo, useSyncExternalStore } from 'react'
import type { NativeManifest } from '@shared/custom-ui-manifest.mjs'
import { useI18n } from '@/app/use-i18n'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { cn } from '@/lib/utils'
import type { Notify } from '@/app/route-context'
import type { ComponentErrorsData, ComponentRow } from '@/features/custom-ui/custom-ui-api'
import { ComponentHost } from '@/features/custom-ui/native/ComponentHost'
import { componentModuleLoader } from '@/features/custom-ui/native/host-runtime'
import type { CommitStore } from './commit-store'
import { useComponentFix } from './use-component-agent'
import { surfaceLabel, targetLabel } from './labels'
import {
  previewFrame,
  previewTargets,
  targetCardSize,
  targetSurface,
  type PreviewTarget,
  type PreviewWindow,
} from './studio-model'

export function PreviewBar({
  manifest,
  target,
  window,
  phone = false,
  onTarget,
  onWindow,
}: {
  manifest: NativeManifest | undefined
  target: PreviewTarget
  window: PreviewWindow
  phone?: boolean
  onTarget: (target: PreviewTarget) => void
  onWindow: (window: PreviewWindow) => void
}) {
  const { t } = useI18n()
  const targets = previewTargets(manifest)
  return (
    <div
      data-studio-preview-bar
      className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2 text-xs text-content-muted"
    >
      <span className={cn(phone && 'sr-only')}>{t('custom-ui:studio.previewPlacement')}</span>
      <SegmentedControl<PreviewTarget>
        size="sm"
        aria-label={t('custom-ui:studio.previewPlacement')}
        value={target}
        onValueChange={onTarget}
        className={cn('max-w-full overflow-x-auto', phone && 'w-full')}
        options={targets.map((value) => ({ value, label: targetLabel(value, t) }))}
      />
      {!phone && (
        <>
          <span className="ml-auto">{t('custom-ui:studio.previewWindow')}</span>
          <SegmentedControl<PreviewWindow>
            size="sm"
            aria-label={t('custom-ui:studio.previewWindow')}
            value={window}
            onValueChange={onWindow}
            options={[
              { value: 'standard', label: t('custom-ui:studio.windowStandard') },
              { value: 'min', label: t('custom-ui:studio.windowMin') },
            ]}
          />
          <span
            className="rounded-sm border border-stroke px-2 py-1 text-content-secondary"
            title={t('custom-ui:studio.themeFollowHint')}
          >
            {t('custom-ui:studio.themeFollow')}
          </span>
        </>
      )}
    </div>
  )
}

export function PreviewStage({
  row,
  target,
  window,
  instanceId,
  title,
  props,
  phone = false,
  notify,
  commits,
}: {
  row: ComponentRow
  target: PreviewTarget
  window: PreviewWindow
  // 选中的放置实例（预览读它的存储与属性）；没有实例时为空，按 preview:<id> 预览。
  instanceId: string | null
  title: string
  props: Record<string, unknown>
  phone?: boolean
  notify: Notify
  commits: CommitStore
}) {
  const { t } = useI18n()
  const fix = useComponentFix(row, notify)
  const frame = previewFrame(target, row.manifest, phone ? 'standard' : window)
  const surface = targetSurface(target)
  // 手机上舞台只有屏幕宽：总览卡片在手机全部代理里本来就是全宽（规格 7.3），其余固定宽度的框不超过舞台。
  const width = frame.width === 'fill' || phone ? '100%' : frame.width
  const maxWidth =
    frame.width === 'fill' || (phone && surface === 'overview-card') ? '100%' : frame.width
  const fill = frame.height === 'fill'
  return (
    <div
      data-component-preview
      data-component-preview-target={target}
      className={cn(
        'flex min-h-0 min-w-0 flex-1 flex-col gap-2 overflow-auto rounded-[var(--r-lg)] border border-stroke bg-surface-subtle bg-[radial-gradient(var(--stroke-strong)_1px,transparent_1px)] [background-size:16px_16px]',
        phone ? 'min-h-[min(420px,55dvh)] p-3' : 'p-5',
      )}
    >
      <p className="flex flex-none items-baseline gap-2 text-xs text-content-muted">
        <span className="text-content-secondary">{surfaceLabel(surface, t)}</span>
        {frame.label && (
          <span data-component-preview-size className="font-mono tabular-nums">
            {/* 手机上总览卡片是全宽（规格 7.3），只标尺寸档，不写桌面的像素宽度。 */}
            {phone && surface === 'overview-card' ? frame.label.split(' · ')[0] : frame.label}
          </span>
        )}
      </p>
      {/* 描边用 ring（不占布局），组件拿到的内容区正好是标注的尺寸，与全部代理卡片 1:1（规格 5.1）。 */}
      <div
        data-component-preview-frame
        className={cn(
          'flex min-w-0 flex-col overflow-hidden rounded-[var(--r-lg)] bg-panel shadow-edge ring-1 ring-stroke',
          fill ? 'min-h-[320px] flex-1' : 'flex-none',
        )}
        style={{
          width,
          maxWidth,
          minWidth: phone || frame.width === 'fill' ? undefined : frame.width,
          height: fill ? undefined : frame.height,
        }}
      >
        <Suspense fallback={null}>
          <ComponentHost
            // 按 rev 与档位重挂：热重载后加载新模块，切换放置时尺寸与 ctx.placement 从头算。
            key={`${row.id}@${row.rev}:${target}:${instanceId ?? ''}`}
            component={row}
            instance={{
              instanceId: instanceId ?? `preview:${row.id}`,
              surface,
              title,
              props,
              size: targetCardSize(target),
            }}
            preview
            notify={(message) => notify(message)}
            onRequestFix={fix}
            onCommitSample={(duration) => commits.set(duration)}
            className="h-full"
          />
        </Suspense>
      </div>
    </div>
  )
}

export function StudioStatusBar({
  componentId,
  errors,
  commits,
}: {
  componentId: string
  errors: ComponentErrorsData | undefined
  commits: CommitStore
}) {
  const { t } = useI18n()
  const duration = useSyncExternalStore(commits.subscribe, commits.get, commits.get)
  const loader = componentModuleLoader()
  const reload = useSyncExternalStore(
    loader.subscribe,
    () => loader.reloadRecommended(componentId),
    () => false,
  )
  const errorCount = useMemo(
    () => errors?.entries.filter((entry) => entry.level === 'error').length ?? 0,
    [errors],
  )
  const hosts = Object.keys(errors?.network.hosts ?? {})
  return (
    <p
      data-studio-status
      className="flex min-w-0 flex-none flex-wrap items-center gap-x-3 gap-y-1 text-xs text-content-muted"
    >
      <span data-studio-status-errors className={cn(errorCount > 0 && 'text-content')}>
        {t('custom-ui:studio.statusErrors', { count: errorCount })}
      </span>
      <span className="font-mono tabular-nums">
        {duration === null
          ? t('custom-ui:studio.statusRenderNone')
          : t('custom-ui:studio.statusRender', { ms: duration })}
      </span>
      <span className="min-w-0 truncate">
        {t('custom-ui:studio.statusNetwork', { count: errors?.network.count ?? 0 })}
        {hosts.length > 0 && <span className="font-mono"> · {hosts.slice(0, 3).join(', ')}</span>}
      </span>
      {reload && <span className="text-content">{t('custom-ui:studio.reloadRecommended')}</span>}
      <span className="ml-auto">{t('custom-ui:studio.statusHotReload')}</span>
    </p>
  )
}
