'use client'

import * as React from 'react'
import { Slider as SliderPrimitive } from 'radix-ui'

import { cn } from '@/lib/utils'

// showSteps：横向分档滑条（如思考等级）在轨道上为每个中间档位刻一道细缝，档位一眼可数；
// 刻缝画在轨道里、滑块之下，不会压住滑块。
// 读屏落在滑块上：aria-label 与 valueText（当前值的文字，如“深度”）转交给滑块，而不是外层容器。
function Slider({
  className,
  defaultValue,
  value,
  min = 0,
  max = 100,
  step = 1,
  showSteps = false,
  valueText,
  'aria-label': ariaLabel,
  ...props
}: React.ComponentProps<typeof SliderPrimitive.Root> & {
  showSteps?: boolean
  valueText?: string
}) {
  const _values = React.useMemo(
    () => (Array.isArray(value) ? value : Array.isArray(defaultValue) ? defaultValue : [min, max]),
    [value, defaultValue, min, max],
  )

  return (
    <SliderPrimitive.Root
      data-slot="slider"
      defaultValue={defaultValue}
      value={value}
      min={min}
      max={max}
      step={step}
      className={cn(
        'relative flex w-full touch-none items-center select-none data-disabled:opacity-50 data-vertical:h-full data-vertical:min-h-40 data-vertical:w-auto data-vertical:flex-col',
        className,
      )}
      {...props}
    >
      <SliderPrimitive.Track
        data-slot="slider-track"
        className="relative grow overflow-hidden rounded-full bg-surface-active data-horizontal:h-1 data-horizontal:w-full data-vertical:h-full data-vertical:w-1"
      >
        <SliderPrimitive.Range
          data-slot="slider-range"
          className="absolute bg-star select-none data-horizontal:h-full data-vertical:w-full"
        />
        {showSteps &&
          max > min &&
          Array.from({ length: Math.max(0, Math.round((max - min) / step) - 1) }, (_, index) => (
            <span
              key={index}
              data-slot="slider-step"
              aria-hidden="true"
              className="absolute top-0 h-full w-0.5 -translate-x-1/2 bg-popover"
              style={{ left: `${(((index + 1) * step) / (max - min)) * 100}%` }}
            />
          ))}
      </SliderPrimitive.Track>
      {Array.from({ length: _values.length }, (_, index) => (
        <SliderPrimitive.Thumb
          data-slot="slider-thumb"
          key={index}
          aria-label={ariaLabel}
          aria-valuetext={valueText}
          className="relative block size-3.5 shrink-0 rounded-full border border-control bg-key shadow-key ring-focus-ring transition-shadow select-none after:absolute after:-inset-2 hover:ring-3 focus-visible:border-focus focus-visible:ring-3 focus-visible:outline-hidden active:ring-3 disabled:pointer-events-none disabled:opacity-50"
        />
      ))}
    </SliderPrimitive.Root>
  )
}

export { Slider }
