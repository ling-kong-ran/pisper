// 属性表单（规格 5.3）：按选中的放置实例编辑“卡片标题”与 manifest 声明的属性。
// 还没有实例时编辑的是“预览属性”，只在内存里、只影响预览。保存时 PATCH，Runtime 再校验一遍，
// 字段级错误（component_props_invalid 的 errors）定位到对应字段；前端不另写一套校验规则。
// 草稿通过 shared 的 validateComponentProps 后才送进预览，避免半输入的值把组件弄崩。
import { useEffect, useId, useMemo, useState, type ReactNode } from 'react'
import { X } from 'lucide-react'
import {
  COMPONENT_INSTANCE_TITLE_MAX,
  validateComponentProps,
} from '@shared/custom-ui-manifest.mjs'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { SegmentedControl } from '@/components/ui/segmented-control'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'
import {
  componentErrorCode,
  componentFieldErrors,
  type ComponentInstance,
  type ComponentRow,
} from '@/features/custom-ui/custom-ui-api'
import { componentErrorText } from './error-text'
import {
  addTags,
  buildPropsFormFields,
  formValuesToProps,
  mapPropsErrors,
  propsToFormValues,
  sameFormValues,
  type PropsFieldError,
  type PropsFormField,
  type PropsFormValue,
  type PropsFormValues,
} from './props-form-model'

export type PreviewDraft = { title: string; props: Record<string, unknown> }

function FieldShell({
  id,
  label,
  description,
  error,
  required,
  children,
  inline = false,
}: {
  id: string
  label: ReactNode
  description?: string
  error?: string
  required?: boolean
  children: ReactNode
  inline?: boolean
}) {
  return (
    <div
      className={cn('flex min-w-0 gap-1.5', inline ? 'items-center justify-between' : 'flex-col')}
      data-props-field={id}
    >
      <span className="flex min-w-0 flex-col gap-0.5">
        <label htmlFor={id} className="text-sm text-content">
          {label}
          {required && <span className="ml-0.5 text-content-muted">*</span>}
        </label>
        {description && (
          <span id={`${id}-hint`} className="text-xs text-content-muted">
            {description}
          </span>
        )}
      </span>
      {children}
      {error && (
        <p id={`${id}-error`} role="alert" data-props-error className="text-xs text-danger">
          {error}
        </p>
      )}
    </div>
  )
}

function TagsInput({
  id,
  value,
  invalid,
  disabled,
  describedBy,
  onChange,
}: {
  id: string
  value: string[]
  invalid: boolean
  disabled: boolean
  describedBy?: string
  onChange: (value: string[]) => void
}) {
  const { t } = useI18n()
  const [text, setText] = useState('')
  const commit = () => {
    if (!text.trim()) return
    onChange(addTags(value, text))
    setText('')
  }
  return (
    <div className="flex flex-col gap-1.5">
      {value.length > 0 && (
        <ul className="flex flex-wrap gap-1">
          {value.map((tag) => (
            <li
              key={tag}
              className="inline-flex max-w-full items-center gap-1 rounded-sm border border-stroke-strong bg-solid py-0.5 pr-0.5 pl-2 text-xs"
            >
              <span className="truncate">{tag}</span>
              <button
                type="button"
                disabled={disabled}
                aria-label={t('custom-ui:studio.removeTag', { tag })}
                className="grid size-5 place-items-center rounded-xs text-content-muted hover:bg-surface-hover hover:text-content"
                onClick={() => onChange(value.filter((item) => item !== tag))}
              >
                <X className="size-3" />
              </button>
            </li>
          ))}
        </ul>
      )}
      <Input
        id={id}
        value={text}
        disabled={disabled}
        aria-invalid={invalid || undefined}
        aria-describedby={describedBy}
        placeholder={t('custom-ui:studio.tagsPlaceholder')}
        onChange={(event) => setText(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === 'Enter' || event.key === ',') {
            event.preventDefault()
            commit()
          } else if (event.key === 'Backspace' && !text && value.length)
            onChange(value.slice(0, -1))
        }}
      />
    </div>
  )
}

function FieldControl({
  field,
  id,
  value,
  error,
  disabled,
  onChange,
}: {
  field: PropsFormField
  id: string
  value: PropsFormValue
  error?: string
  disabled: boolean
  onChange: (value: PropsFormValue) => void
}) {
  const { t } = useI18n()
  const describedBy =
    [field.description ? `${id}-hint` : '', error ? `${id}-error` : ''].filter(Boolean).join(' ') ||
    undefined
  const common = {
    id,
    disabled,
    'aria-invalid': error ? true : undefined,
    'aria-describedby': describedBy,
  } as const
  switch (field.kind) {
    case 'switch':
      return (
        <Switch
          {...common}
          checked={value === true}
          onCheckedChange={(checked) => onChange(checked)}
        />
      )
    case 'segmented':
      return (
        <SegmentedControl<string>
          size="sm"
          aria-label={field.label}
          disabled={disabled}
          value={typeof value === 'string' && value ? value : (field.defaultValue ?? '')}
          onValueChange={(next) => onChange(next)}
          options={field.options.map((option) => ({ value: option, label: option }))}
        />
      )
    case 'select':
      return (
        <Select
          value={typeof value === 'string' && value ? value : undefined}
          disabled={disabled}
          onValueChange={(next) => onChange(next)}
        >
          <SelectTrigger {...common} className="w-full">
            <SelectValue placeholder={t('custom-ui:studio.selectPlaceholder')} />
          </SelectTrigger>
          <SelectContent>
            {field.options.map((option) => (
              <SelectItem key={option} value={option}>
                {option}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )
    case 'tags':
      return (
        <TagsInput
          id={id}
          value={Array.isArray(value) ? value : []}
          invalid={Boolean(error)}
          disabled={disabled}
          describedBy={describedBy}
          onChange={onChange}
        />
      )
    case 'textarea':
      return (
        <Textarea
          {...common}
          rows={3}
          maxLength={field.maxLength}
          value={typeof value === 'string' ? value : ''}
          onChange={(event) => onChange(event.target.value)}
        />
      )
    case 'number':
      return (
        <span className="flex items-center gap-2">
          <Input
            {...common}
            inputMode={field.integer ? 'numeric' : 'decimal'}
            value={typeof value === 'string' ? value : ''}
            placeholder={field.defaultValue !== undefined ? String(field.defaultValue) : undefined}
            className="font-mono tabular-nums"
            onChange={(event) => onChange(event.target.value)}
          />
          {field.unit && <span className="flex-none text-xs text-content-muted">{field.unit}</span>}
        </span>
      )
    case 'color':
      return (
        <span className="flex items-center gap-2">
          <span
            aria-hidden
            className="size-6 flex-none rounded-sm border border-stroke-strong"
            style={{ background: typeof value === 'string' && value ? value : 'transparent' }}
          />
          <Input
            {...common}
            maxLength={field.maxLength}
            value={typeof value === 'string' ? value : ''}
            placeholder="#RRGGBB"
            className="font-mono"
            onChange={(event) => onChange(event.target.value)}
          />
        </span>
      )
    default:
      return (
        <Input
          {...common}
          type={field.kind === 'url' ? 'url' : 'text'}
          maxLength={field.maxLength}
          value={typeof value === 'string' ? value : ''}
          className={cn(field.kind === 'url' && 'font-mono')}
          onChange={(event) => onChange(event.target.value)}
        />
      )
  }
}

export function PropsForm({
  row,
  instance,
  notify,
  onPreview,
  onSave,
}: {
  row: ComponentRow
  // 正在编辑的放置实例；为空时编辑的是“预览属性”（只在内存里）。父组件按实例换 key 重建表单。
  instance: ComponentInstance | null
  notify: (message: string) => void
  onPreview: (draft: PreviewDraft) => void
  onSave: (
    instance: ComponentInstance,
    patch: { title: string; props: Record<string, unknown> },
  ) => Promise<unknown>
}) {
  const { t, language } = useI18n()
  const baseId = useId()
  const schema = row.manifest?.props
  const fields = useMemo(() => buildPropsFormFields(schema, language), [language, schema])
  const baseline = useMemo(() => propsToFormValues(fields, instance?.props), [fields, instance])
  const [values, setValues] = useState<PropsFormValues>(baseline)
  const [title, setTitle] = useState(instance?.title ?? '')
  const [errors, setErrors] = useState<Record<string, PropsFieldError>>({})
  const [titleError, setTitleError] = useState('')
  const [busy, setBusy] = useState(false)
  const dirty = !sameFormValues(values, baseline) || title !== (instance?.title ?? '')

  // 草稿 → 预览：通过 shared 校验的才送进组件（与 Runtime 同一套规则）。
  useEffect(() => {
    const result = validateComponentProps(schema, formValuesToProps(fields, values))
    if (!result.errors.length) onPreview({ title, props: result.value })
  }, [fields, onPreview, schema, title, values])

  const setField = (key: string, value: PropsFormValue) => {
    setValues((current) => ({ ...current, [key]: value }))
    setErrors((current) => {
      if (!Object.hasOwn(current, key)) return current
      const next = { ...current }
      delete next[key]
      return next
    })
  }

  const save = async () => {
    if (!instance) return
    setBusy(true)
    setErrors({})
    setTitleError('')
    try {
      await onSave(instance, { title: title.trim(), props: formValuesToProps(fields, values) })
      notify(t('custom-ui:studio.propsSaved'))
    } catch (error) {
      const code = componentErrorCode(error)
      if (code === 'component_props_invalid') {
        const mapped = mapPropsErrors(componentFieldErrors(error))
        setErrors(mapped)
        // 焦点移到第一个出错的字段，读屏随即读出错误（aria-describedby）。
        const first = fields.find((field) => Object.hasOwn(mapped, field.key))
        if (first) document.getElementById(`${baseId}-${first.key}`)?.focus()
      } else if (code === 'component_title_invalid') {
        setTitleError(componentErrorText(code, t))
      } else {
        notify(
          code
            ? `${t('custom-ui:studio.saveFailed')} ${componentErrorText(code, t)}`
            : t('custom-ui:studio.saveFailed'),
        )
      }
    } finally {
      setBusy(false)
    }
  }

  const fieldError = (key: string) => {
    const error = errors[key]
    return error ? componentErrorText(error.code, t, error.params) : undefined
  }
  const plaintext = fields.some((field) => field.kind === 'url' || field.kind === 'textarea')
  const titleId = `${baseId}-title`
  return (
    <form
      data-props-form={instance?.instanceId ?? 'preview'}
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault()
        void save()
      }}
    >
      {!instance && (
        <p className="text-xs text-content-muted">{t('custom-ui:studio.previewPropsHint')}</p>
      )}
      <FieldShell id={titleId} label={t('custom-ui:studio.cardTitle')} error={titleError}>
        <Input
          id={titleId}
          value={title}
          maxLength={COMPONENT_INSTANCE_TITLE_MAX}
          disabled={busy}
          aria-invalid={titleError ? true : undefined}
          placeholder={t('custom-ui:studio.cardTitlePlaceholder')}
          onChange={(event) => {
            setTitle(event.target.value)
            setTitleError('')
          }}
        />
      </FieldShell>
      {fields.map((field) => {
        const id = `${baseId}-${field.key}`
        const error = fieldError(field.key)
        return (
          <FieldShell
            key={field.key}
            id={id}
            label={field.label}
            description={field.description}
            required={field.required}
            error={error}
            inline={field.kind === 'switch'}
          >
            <FieldControl
              field={field}
              id={id}
              value={values[field.key] ?? ''}
              error={error}
              disabled={busy}
              onChange={(value) => setField(field.key, value)}
            />
          </FieldShell>
        )
      })}
      {errors[''] && (
        <p role="alert" className="text-xs text-danger">
          {componentErrorText(errors[''].code, t, errors[''].params)}
        </p>
      )}
      {plaintext && (
        <p className="text-xs text-content-muted">{t('custom-ui:studio.propsPlaintext')}</p>
      )}
      {instance && (
        <div className="flex justify-end gap-2">
          <Button
            type="button"
            size="sm"
            variant="ghost"
            disabled={busy || !dirty}
            onClick={() => {
              setValues(baseline)
              setTitle(instance.title)
              setErrors({})
              setTitleError('')
            }}
          >
            {t('custom-ui:studio.propsReset')}
          </Button>
          <Button type="submit" size="sm" disabled={busy || !dirty} data-props-save>
            {t('custom-ui:studio.propsSave')}
          </Button>
        </div>
      )}
    </form>
  )
}
