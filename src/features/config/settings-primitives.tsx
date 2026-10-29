// 设置页共享原语：SettingsCard / Field 等分组卡片组件，
// 保证各配置分区视觉一致。
import type { ComponentProps, ReactNode } from 'react'
import {
  AppCard,
  AppSectionTitle,
  AppSwitch,
  StatusBadge,
  type StatusBadgeTone,
} from '@/components/ui/app-primitives'
import { cn } from '@/lib/utils'

export type SettingsBadgeTone = StatusBadgeTone

export const SettingsCard = AppCard
export const SettingsSectionTitle = AppSectionTitle
export const SettingsBadge = StatusBadge

export function SettingsSwitch(props: ComponentProps<typeof AppSwitch>) {
  return <AppSwitch density="compact" {...props} />
}

// 细线分组卡：1px 描边、r-md、行间只用柔和细线；行本身（SettingsRow 等）不再各自带框，
// 这样同一卡片里的设置读起来像仪表面板而不是一摞小卡片。
// @container/card 供卡内表单在窄宽度下改为上下堆叠。
export function SettingsGroup({ className, ...props }: ComponentProps<'div'>) {
  return (
    <div
      className={cn(
        '@container/card min-w-0 divide-y divide-stroke-soft rounded-md border border-stroke bg-panel px-4',
        className,
      )}
      {...props}
    />
  )
}

// 标签在左的表单行：卡片足够宽时为 168px 标签列 + 字段列；窄卡（手机）上下堆叠。
// 标签只含标签文字，说明文字单独成段并通过 aria-describedby 关联，保证可访问名称精确。
export function SettingsField({
  label,
  help,
  htmlFor,
  labelId,
  helpId,
  children,
  className,
}: {
  label: ReactNode
  help?: ReactNode
  htmlFor?: string
  labelId?: string
  helpId?: string
  children: ReactNode
  className?: string
}) {
  return (
    <div
      className={cn(
        'grid min-w-0 gap-1.5 py-2.5 @min-[480px]/card:grid-cols-[168px_minmax(0,1fr)] @min-[480px]/card:items-center @min-[480px]/card:gap-4',
        className,
      )}
    >
      <div className="min-w-0">
        <label id={labelId} htmlFor={htmlFor} className="text-sm font-medium text-content">
          {label}
        </label>
        {help && (
          <p id={helpId} className="mt-0.5 text-xs text-content-muted">
            {help}
          </p>
        )}
      </div>
      <div className="min-w-0 @min-[480px]/card:max-w-[460px]">{children}</div>
    </div>
  )
}

// 卡片标题区：键帽图标（启用时图标用强调文字色）+ 15/500 标题 + 12px 说明，右侧可放徽标或开关。
// 中文 600 会渲染成发重的粗体，600 只留给页面标题；层级靠字号与颜色拉开。
// 字号带 !：AppCard 的 [&_h2]:text-base 会把卡内 h2 压回 14px，与分组标题的 15px 不一致。
export function SettingsCardHeader({
  icon,
  title,
  description,
  action,
  active = false,
  className,
}: {
  icon: ReactNode
  title: ReactNode
  description?: ReactNode
  action?: ReactNode
  active?: boolean
  className?: string
}) {
  return (
    <div className={cn('flex min-w-0 flex-wrap items-start gap-x-3 gap-y-2', className)}>
      <span
        aria-hidden="true"
        className={cn(
          'grid size-9.5 shrink-0 place-items-center rounded-lg border border-key-border bg-key shadow-key',
          active ? 'text-accent-fg' : 'text-content-muted',
        )}
      >
        {icon}
      </span>
      <div className="min-w-0 flex-1 basis-48">
        <h2 className="text-[15px]! leading-6 font-medium text-content">{title}</h2>
        {description && (
          <p className="mt-0.5 text-xs leading-[1.55] text-content-muted">{description}</p>
        )}
      </div>
      {action && <div className="ml-auto flex shrink-0 items-center gap-3">{action}</div>}
    </div>
  )
}
