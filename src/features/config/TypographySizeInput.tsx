import { useEffect, useId, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { Input } from '@/components/ui/input'
import { CODE_FONT_SIZE, parseFontSizeDraft, UI_FONT_SIZE } from '@/lib/typography-preferences'

export function TypographySizeInput({
  role,
  label,
  value,
  onCommit,
}: {
  role: 'ui' | 'code'
  label: string
  value: number
  onCommit: (value: number) => void
}) {
  const { t } = useI18n()
  const errorId = useId()
  const [draft, setDraft] = useState(String(value))
  const draftRef = useRef(draft)
  const committed = useRef(value)
  const range = role === 'ui' ? UI_FONT_SIZE : CODE_FONT_SIZE
  useEffect(() => {
    committed.current = value
    draftRef.current = String(value)
    setDraft(String(value))
  }, [value])
  const invalid = draft !== String(value) && parseFontSizeDraft(draft, role) === null
  const commit = () => {
    const next = parseFontSizeDraft(draftRef.current, role)
    // pointerUp、Enter 与随后的 blur 可能连续触发，只提交一次明确的新值。
    if (next === null || next === committed.current) return
    onCommit(next)
    committed.current = next
  }
  return (
    <div className="flex max-w-full flex-col items-end gap-1">
      <div className="flex items-center gap-2">
        <Input
          type="number"
          min={range.min}
          max={range.max}
          step={1}
          value={draft}
          aria-label={label}
          aria-invalid={invalid}
          aria-describedby={invalid ? errorId : undefined}
          className="h-8 w-16 text-sm"
          onChange={(event) => {
            draftRef.current = event.target.value
            setDraft(event.target.value)
          }}
          onPointerUp={commit}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return
            if (event.key === 'Enter') {
              event.preventDefault()
              commit()
            } else if (event.key === 'Escape') {
              event.preventDefault()
              event.stopPropagation()
              draftRef.current = String(value)
              setDraft(String(value))
            }
          }}
        />
        <span className="text-sm text-muted-foreground">px</span>
      </div>
      {invalid && (
        <p id={errorId} role="alert" className="max-w-56 text-xs text-destructive">
          {t('config:interfaceSettings.invalidFontSize', { min: range.min, max: range.max })}
        </p>
      )}
    </div>
  )
}
