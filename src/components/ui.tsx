import {
  forwardRef,
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type HTMLAttributes,
  type ReactNode,
} from 'react'
import { AlertTriangle, CheckCircle2, ChevronDown, Eye, Info, X } from 'lucide-react'
import { useI18n } from '../app/use-i18n'
import type { AppDialogState } from '../hooks/useAppDialog'
import { AppSelect } from './AppSelect'

const BADGE_TONES = {
  blue: 'bg-[var(--brand-blue-soft)] text-[var(--brand-blue-strong)]',
  green: 'bg-[var(--success-soft)] text-[var(--success-strong)]',
  red: 'bg-[var(--danger-soft)] text-[var(--danger)]',
  amber: 'bg-[var(--warning-soft)] text-[var(--warning-strong)]',
  gray: 'bg-[var(--surface-muted)] text-[var(--text-muted)]',
} as const
type BadgeTone = keyof typeof BADGE_TONES

const TOAST_TONES = {
  success: {
    Icon: CheckCircle2,
    classes: 'border-[var(--success-border)] bg-[var(--success-soft)] text-[var(--success-strong)]',
  },
  error: {
    Icon: AlertTriangle,
    classes: 'border-[var(--danger-border)] bg-[var(--danger-soft)] text-[var(--danger)]',
  },
  info: {
    Icon: Info,
    classes: 'border-[var(--stroke)] bg-[var(--surface-muted)] text-[var(--text-soft)]',
  },
} as const
export type ToastTone = keyof typeof TOAST_TONES

export const Panel = forwardRef<HTMLElement, HTMLAttributes<HTMLElement>>(function Panel(
  { children, className = '', ...props },
  ref,
) {
  return (
    <section
      ref={ref}
      className={`panel min-w-0 rounded-[10px] border border-[var(--stroke)] bg-[var(--panel)] p-3.5 shadow-[0_12px_30px_-25px_var(--shadow)] backdrop-blur-[14px] ${className}`}
      {...props}
    >
      {children}
    </section>
  )
})

export function SectionTitle({ title }: { title: ReactNode }) {
  return <h3 className="section-title text-[13px] font-bold text-[var(--text-soft)]">{title}</h3>
}

export function Badge({ children, tone = 'blue' }: { children: ReactNode; tone?: BadgeTone }) {
  return (
    <span
      className={`badge inline-flex min-h-6 items-center whitespace-nowrap rounded-full px-2 text-[11px] leading-4 font-bold ${BADGE_TONES[tone] || BADGE_TONES.blue}`}
    >
      {children}
    </span>
  )
}

export function Metric({
  value,
  label,
  note,
  tone = '',
}: {
  value: ReactNode
  label: ReactNode
  note: ReactNode
  tone?: string
}) {
  return (
    <Panel className={`metric ${tone}`}>
      <small>{label}</small>
      <strong>{value}</strong>
      <span>{note}</span>
    </Panel>
  )
}

export function Toggle({
  defaultOn = false,
  value,
  onChange,
  disabled = false,
  ariaLabel,
  title,
}: {
  defaultOn?: boolean
  value?: boolean
  onChange?: (value: boolean) => void
  disabled?: boolean
  ariaLabel?: string
  title?: string
}) {
  const [internal, setInternal] = useState(defaultOn)
  const on = value ?? internal
  return (
    <button
      type="button"
      aria-pressed={on}
      aria-label={ariaLabel}
      title={title}
      disabled={disabled}
      className={`toggle relative inline-flex h-8 w-12 min-w-12 items-center rounded-full border-0 p-1 transition disabled:cursor-not-allowed disabled:opacity-45 ${on ? 'on bg-[var(--star)]' : 'bg-[var(--control-muted)]'}`}
      onClick={(event) => {
        event.stopPropagation()
        const next = !on
        if (value === undefined) setInternal(next)
        onChange?.(next)
      }}
    >
      <i
        className={`block size-6 rounded-full bg-[var(--control-thumb)] shadow-sm transition ${on ? 'translate-x-4' : ''}`}
      />
    </button>
  )
}

export function Segmented({
  options,
  value,
  onChange,
  compact = false,
}: {
  options: string[]
  value: string
  onChange: (value: string) => void
  compact?: boolean
}) {
  return (
    <div
      className={`segmented flex min-h-9 items-center gap-0.5 overflow-x-auto rounded-lg p-1 [scrollbar-width:none] ${compact ? 'compact w-auto border border-[var(--stroke)] bg-transparent' : 'bg-[var(--surface-muted)]'}`}
    >
      {options.map((option) => (
        <button
          className={`h-8 min-w-14 flex-none rounded-md border-0 px-3 text-[12px] font-semibold ${value === option ? 'active bg-[var(--solid)] text-[var(--text)] shadow-sm' : 'bg-transparent text-[var(--text-muted)]'}`}
          onClick={() => onChange(option)}
          key={option}
        >
          {option}
        </button>
      ))}
    </div>
  )
}

export function InputLabel({
  label,
  value,
  secret = false,
  placeholder,
}: {
  label: ReactNode
  value: string
  secret?: boolean
  placeholder?: string
}) {
  const [current, setCurrent] = useState(value)
  const [show, setShow] = useState(false)
  return (
    <label className="field-label">
      {label}
      <span className="input-wrap">
        <input
          type={secret && !show ? 'password' : 'text'}
          value={current}
          placeholder={placeholder}
          onChange={(event) => setCurrent(event.target.value)}
        />
        {secret && (
          <button type="button" onClick={() => setShow(!show)}>
            <Eye size={14} />
          </button>
        )}
      </span>
    </label>
  )
}

export function SelectLabel({ label, options }: { label: ReactNode; options: string[] }) {
  return (
    <label className="field-label">
      {label}
      <span className="select-wrap">
        <AppSelect>
          {options.map((option) => (
            <option key={option}>{option}</option>
          ))}
        </AppSelect>
        <ChevronDown size={13} />
      </span>
    </label>
  )
}

export function SelectField({
  value,
  onChange,
  options,
}: {
  value: string
  onChange: (value: string) => void
  options: string[]
}) {
  return (
    <span className="select-wrap standalone">
      <AppSelect value={value} onChange={(event) => onChange(event.target.value)}>
        {options.map((option) => (
          <option key={option}>{option}</option>
        ))}
      </AppSelect>
      <ChevronDown size={13} />
    </span>
  )
}

export function Toast({ message, tone = 'success' }: { message: ReactNode; tone?: ToastTone }) {
  const config = TOAST_TONES[tone] || TOAST_TONES.success
  const Icon = config.Icon
  return (
    <div
      role={tone === 'error' ? 'alert' : 'status'}
      aria-live="polite"
      className={`fixed right-5 bottom-10 z-80 flex min-h-11 max-w-[min(420px,calc(100vw-40px))] items-center gap-2 rounded-[9px] border px-3.5 py-2.5 text-[13px] leading-5 font-semibold shadow-[0_18px_36px_-18px_var(--shadow-strong)] [animation:toast-in_.22s_ease-out] ${config.classes}`}
    >
      <Icon size={18} className="shrink-0" />
      {message}
    </div>
  )
}

export function PreviewNotice({ children }: { children?: ReactNode }) {
  const { t } = useI18n()
  const content = children || t('common:ui.demoInterfaceRuntimeNotConnected')
  return (
    <div className="mb-3 flex min-h-10 items-center gap-2 rounded-lg border border-[var(--warning-border)] bg-[var(--warning-soft)] px-3 text-[12px] font-semibold text-[var(--warning-strong)]">
      <Info size={16} className="shrink-0" />
      {content}
    </div>
  )
}

export function AppDialog({
  dialog,
  onClose,
  onFinish,
}: {
  dialog: AppDialogState | null
  onClose: () => void
  onFinish: (value: string | boolean | null) => void
}) {
  const { t } = useI18n()
  const [value, setValue] = useState(dialog?.value || '')
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    setValue(dialog?.value || '')
    if (dialog?.type === 'prompt') window.setTimeout(() => inputRef.current?.select(), 0)
  }, [dialog])

  useEffect(() => {
    if (!dialog) return undefined
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [dialog, onClose])

  if (!dialog) return null
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    onFinish(dialog.type === 'prompt' ? value.trim() : true)
  }
  return (
    <div
      className="fixed inset-0 z-70 grid place-items-center bg-[var(--modal-overlay)] p-5 backdrop-blur-[3px] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form
        role="dialog"
        aria-modal="true"
        aria-labelledby="app-dialog-title"
        className="w-full max-w-[430px] rounded-[13px] border border-[var(--surface-highlight)] bg-[var(--solid)] p-[18px] text-[var(--text)] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)]"
        onSubmit={submit}
      >
        <div className="flex items-start justify-between gap-3">
          <div>
            <h2 id="app-dialog-title" className="text-[17px] leading-6 font-bold">
              {dialog.title}
            </h2>
            {dialog.message && (
              <p className="mt-1.5 text-[13px] leading-5 text-[var(--text-muted)]">
                {dialog.message}
              </p>
            )}
          </div>
          <button
            type="button"
            aria-label={t('common:ui.closeDialog')}
            className="grid size-8 shrink-0 place-items-center rounded-lg border-0 bg-transparent text-[var(--text-muted)] hover:bg-[var(--surface-muted)]"
            onClick={onClose}
          >
            <X size={18} />
          </button>
        </div>
        {dialog.type === 'prompt' && (
          <label className="mt-4 block text-[12px] font-semibold text-[var(--text-soft)]">
            {dialog.inputLabel || t('common:ui.name')}
            <input
              ref={inputRef}
              className="mt-2 h-10 w-full rounded-lg border border-[var(--stroke)] bg-[var(--solid)] px-3 text-[13px] outline-none focus:border-[var(--star)] focus:ring-3 focus:ring-[var(--accent-ring)]"
              value={value}
              placeholder={dialog.placeholder}
              maxLength={dialog.maxLength || 120}
              onChange={(event) => setValue(event.target.value)}
            />
          </label>
        )}
        <div className="mt-[18px] flex justify-end gap-2">
          <button type="button" className="button secondary" onClick={onClose}>
            {t('common:ui.cancel')}
          </button>
          <button
            type="submit"
            disabled={dialog.type === 'prompt' && !value.trim()}
            className={`button ${dialog.tone === 'danger' ? 'danger' : 'primary'}`}
          >
            {dialog.confirmLabel || t('common:ui.confirm')}
          </button>
        </div>
      </form>
    </div>
  )
}
