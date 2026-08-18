// 统一的 Select 封装：把 shadcn 的受控 Select 转成原生 <select> 语义，
// 支持必填/禁用/无值占位，供表单与设置页复用，避免各处重复状态逻辑。
import {
  Children,
  isValidElement,
  type ChangeEvent,
  type ReactElement,
  type ReactNode,
} from 'react'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { cn } from '@/lib/utils'

const EMPTY_VALUE = '__pisper_empty_value__'

type OptionProps = {
  children?: ReactNode
  disabled?: boolean
  value?: string | number
}

type AppSelectProps = {
  children: ReactNode
  className?: string
  disabled?: boolean
  onChange?: (event: ChangeEvent<HTMLSelectElement>) => void
  value?: string | number | null
  'aria-label'?: string
  'aria-labelledby'?: string
}

export function AppSelect({
  children,
  className,
  disabled,
  onChange,
  value,
  ...ariaProps
}: AppSelectProps) {
  const options = Children.toArray(children)
    .filter(isValidElement<OptionProps>)
    .map((option: ReactElement<OptionProps>) => {
      const optionValue = option.props.value ?? option.props.children ?? ''
      return {
        disabled: option.props.disabled,
        label: option.props.children,
        value: String(optionValue) || EMPTY_VALUE,
      }
    })

  const selectedValue = String(value ?? '') || EMPTY_VALUE

  return (
    <Select
      value={selectedValue}
      disabled={disabled}
      onValueChange={(nextValue) => {
        const normalized = nextValue === EMPTY_VALUE ? '' : nextValue
        onChange?.({ target: { value: normalized } } as ChangeEvent<HTMLSelectElement>)
      }}
    >
      <SelectTrigger
        className={cn(
          'h-[31px] min-h-0 w-full rounded-[var(--r-xs)] border border-[var(--stroke)] bg-[var(--surface-subtle)] px-2.5 py-0 text-[12px] font-normal text-[var(--text)] shadow-none focus-visible:border-[var(--focus)] focus-visible:ring-2 focus-visible:ring-[var(--focus-ring)] dark:bg-[var(--solid)] [&>svg]:size-[13px] [&>svg]:text-[var(--text-muted)]',
          className,
        )}
        {...ariaProps}
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent
        position="popper"
        align="start"
        className="min-w-[var(--radix-select-trigger-width)]"
      >
        {options.map((option) => (
          <SelectItem
            key={option.value}
            value={option.value}
            disabled={option.disabled}
            className="text-[12px]"
          >
            {option.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}
