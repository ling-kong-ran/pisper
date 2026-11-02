// 档位滑条：思考等级这类有序档位的选择器（会话页与新任务共用同一个模型芯片弹层）。
// 基于 shadcn Slider（Radix）：键盘、读屏、触控语义由它提供；这里只负责档位名与提交时机。
// 拖动时只更新草稿，松手或键盘停下时才提交一次，避免一次拖动触发多次切换请求。
// 视觉取“低调”：细轨道、每档一道刻缝、由浅到深的渐变填充、滑块中心一粒实心点。
import { useEffect, useRef, useState } from 'react'
import { Slider } from '@/components/ui/slider'
import { cn } from '@/lib/utils'

type LevelSliderProps = {
  levels: string[]
  // 当前已生效的档位；不在 levels 里时滑块停在最左。
  value: string
  labelFor: (level: string) => string
  onCommit: (level: string) => void | Promise<void>
  disabled?: boolean
  ariaLabel: string
  // 标题行左侧文字；右侧显示当前档位名。
  title?: React.ReactNode
  // 标题行右侧当前档位后面的附加内容（例如“恢复默认”）。
  trailing?: React.ReactNode
  className?: string
}

export function LevelSlider({
  levels,
  value,
  labelFor,
  onCommit,
  disabled = false,
  ariaLabel,
  title,
  trailing,
  className,
}: LevelSliderProps) {
  const [draft, setDraft] = useState<number | null>(null)
  const [saving, setSaving] = useState(false)
  const savingRef = useRef(false)
  const index = Math.max(0, levels.indexOf(value))
  const selected = draft ?? index
  const currentLabel = labelFor(levels[selected] || value)
  const inactive = disabled || saving || levels.length < 2
  const levelsKey = levels.join('|')
  useEffect(() => {
    setDraft(null)
  }, [value, levelsKey])

  const commit = async (nextIndex: number) => {
    const level = levels[nextIndex]
    if (inactive || savingRef.current || !level || level === value) {
      setDraft(null)
      return
    }
    savingRef.current = true
    setSaving(true)
    try {
      await onCommit(level)
    } finally {
      savingRef.current = false
      setSaving(false)
      setDraft(null)
    }
  }

  return (
    <div className={cn('min-w-0', className)}>
      <div className="flex items-baseline justify-between gap-2 px-0.5 pb-1">
        {title && <span className="text-xs font-medium text-content-muted">{title}</span>}
        <span className="ml-auto flex min-w-0 items-baseline gap-2">
          <span
            className="truncate text-[13px] leading-5 font-medium text-accent-fg"
            aria-live="polite"
          >
            {currentLabel}
          </span>
          {trailing}
        </span>
      </div>
      <Slider
        className={cn(
          'h-7',
          // 填充由浅到深；滑块 16px，键面中心一粒实心点。
          '[&_[data-slot=slider-range]]:bg-[linear-gradient(90deg,var(--star-border),var(--star))]',
          '[&_[data-slot=slider-thumb]]:size-4 [&_[data-slot=slider-thumb]]:bg-[radial-gradient(circle,var(--star)_0_2px,var(--key-bg)_2.5px)]',
        )}
        showSteps
        min={0}
        max={Math.max(1, levels.length - 1)}
        step={1}
        value={[selected]}
        disabled={inactive}
        aria-label={ariaLabel}
        valueText={currentLabel}
        onValueChange={([next]) => setDraft(next)}
        onValueCommit={([next]) => void commit(next)}
      />
      {levels.length > 1 && (
        <div
          className="flex justify-between px-0.5 pt-0.5 text-xs text-content-faint"
          aria-hidden="true"
        >
          <span>{labelFor(levels[0])}</span>
          <span>{labelFor(levels.at(-1) || '')}</span>
        </div>
      )}
    </div>
  )
}
