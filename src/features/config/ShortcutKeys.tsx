// 快捷键键帽：一种按法里的键逐个画成键帽，几种可互换的按法之间用“/”隔开。
// 设置页的固定别名行与 ⌘/ 速查共用；未绑定时显示“未绑定”。
import { Fragment } from 'react'
import { useI18n } from '@/app/use-i18n'
import { Kbd, KbdGroup } from '@/components/ui/kbd'
import { cn } from '@/lib/utils'

export function ShortcutKeys({
  keys,
  className,
}: {
  keys: readonly (readonly string[])[] | null
  className?: string
}) {
  const { t } = useI18n()
  if (!keys?.length)
    return (
      <span className={cn('text-xs text-content-muted', className)}>
        {t('config:shortcuts.disabled')}
      </span>
    )
  return (
    <span className={cn('inline-flex min-w-0 flex-wrap items-center gap-1', className)}>
      {keys.map((combo, index) => (
        <Fragment key={combo.join('+')}>
          {index > 0 && (
            <span aria-hidden="true" className="text-xs text-content-muted">
              /
            </span>
          )}
          <KbdGroup>
            {combo.map((key) => (
              <Kbd key={key} className="text-content-secondary">
                {key}
              </Kbd>
            ))}
          </KbdGroup>
        </Fragment>
      ))}
    </span>
  )
}
