// 外观偏好由 UI Store 持有；页面只管理可取消的草稿与折叠状态。
import { useEffect, useId, useRef, useState, type PointerEvent } from 'react'
import {
  Activity,
  ChevronDown,
  FoldVertical,
  MonitorCog,
  Moon,
  RotateCcw,
  Sparkles,
  Sun,
  UnfoldVertical,
  type LucideIcon,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { normalizeHexColor } from '@/lib/custom-accent'
import { cn } from '@/lib/utils'
import { SettingsRow, SettingsSection } from '@/components/app/SettingsRow'
import { Button } from '@/components/ui/button'
import { RadioGroup, RadioGroupOption } from '@/components/ui/radio-group'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Switch } from '@/components/ui/switch'
import {
  Select,
  SelectTrigger,
  SelectContent,
  SelectItem,
  SelectValue,
} from '@/components/ui/select'
import {
  DEFAULT_UI_PREFERENCES,
  useUiStore,
  type AccentPreset,
  type DensityMode,
  type MotionMode,
  type RadiusMode,
  type ThemeMode,
} from '@/stores/ui-store'
import { LanguageSettings } from './LanguageSettings'
import { TypographySizeInput } from './TypographySizeInput'
import type { Notify } from '@/app/route-context'

type Choice<T extends string> = { value: T; label: string; description: string; icon: LucideIcon }

function CompactChoices<T extends string>({
  label,
  options,
  value,
  onChange,
}: {
  label: string
  options: Array<{ value: T; label: string }>
  value: T
  onChange: (value: T) => void
}) {
  return (
    <RadioGroup
      value={value}
      onValueChange={(next) => {
        const option = options.find((item) => item.value === next)
        if (option) onChange(option.value)
      }}
      className="grid auto-cols-fr grid-flow-col gap-1 rounded-[var(--r-sm)] bg-[var(--surface-muted)] p-1"
      aria-label={label}
    >
      {options.map((option) => (
        <RadioGroupOption
          type="button"
          className={`min-h-9 max-[650px]:min-h-11 rounded-lg px-2 text-sm font-normal transition ${option.value === value ? 'bg-[var(--solid)] text-[var(--text)] shadow-sm' : 'text-[var(--text-muted)] hover:text-[var(--text)]'}`}
          value={option.value}
          key={option.value}
        >
          {option.label}
        </RadioGroupOption>
      ))}
    </RadioGroup>
  )
}

const ACCENT_SWATCHES: Record<AccentPreset, string> = {
  neutral: '#18181b',
  blue: '#1677e8',
  teal: '#0f766e',
  violet: '#7557c8',
  coral: '#c94f5d',
  custom: '#1677e8',
}

type Hsv = { hue: number; saturation: number; brightness: number }
function colorHsv(hex: string): Hsv {
  const [red, green, blue] = [1, 3, 5].map(
    (offset) => parseInt(hex.slice(offset, offset + 2), 16) / 255,
  )
  const max = Math.max(red, green, blue)
  const delta = max - Math.min(red, green, blue)
  let hue = 0
  if (delta)
    hue =
      max === red
        ? ((green - blue) / delta) % 6
        : max === green
          ? (blue - red) / delta + 2
          : (red - green) / delta + 4
  return { hue: (hue * 60 + 360) % 360, saturation: max ? delta / max : 0, brightness: max }
}
function colorHex({ hue, saturation, brightness }: Hsv) {
  const chroma = brightness * saturation
  const x = chroma * (1 - Math.abs(((hue / 60) % 2) - 1))
  const sectors = [
    [chroma, x, 0],
    [x, chroma, 0],
    [0, chroma, x],
    [0, x, chroma],
    [x, 0, chroma],
    [chroma, 0, x],
  ]
  return (
    '#' +
    sectors[Math.floor(hue / 60) % 6]
      .map((channel) =>
        Math.round((channel + brightness - chroma) * 255)
          .toString(16)
          .padStart(2, '0'),
      )
      .join('')
  )
}

function AccentColorPicker({
  value,
  onCommit,
}: {
  value: string
  onCommit: (hex: string) => void
}) {
  const { t } = useI18n()
  const [draft, setDraft] = useState(value)
  const draftRef = useRef(value)
  const [hsv, setHsv] = useState(() => colorHsv(value))
  const hintId = useId()
  const changeDraft = (hex: string) => {
    draftRef.current = hex
    setDraft(hex)
  }
  useEffect(() => {
    draftRef.current = value
    setDraft(value)
    setHsv(colorHsv(value))
  }, [value])
  const changeColor = (next: Hsv) => {
    setHsv(next)
    changeDraft(colorHex(next))
  }
  const commit = () => {
    const normalized = normalizeHexColor(draftRef.current)
    if (normalized && normalized !== normalizeHexColor(value)) onCommit(normalized)
  }
  const point = (event: PointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect()
    changeColor({
      ...hsv,
      saturation: Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width)),
      brightness: 1 - Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height)),
    })
  }
  const invalid = Boolean(draft.trim()) && !normalizeHexColor(draft)
  return (
    <Popover
      onOpenChange={(open) => {
        if (!open) {
          commit()
          if (!normalizeHexColor(draftRef.current)) {
            changeDraft(value)
            setHsv(colorHsv(value))
          }
        }
      }}
    >
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          className="h-7 min-h-0 w-24 gap-1.5 rounded-full px-2 text-xs"
          aria-label={t('config:interfaceSettings.customAccentColor')}
        >
          <span
            aria-hidden="true"
            className="size-3 shrink-0 rounded-full border border-black/10"
            style={{ backgroundColor: value }}
          />
          <span className="font-mono">{value}</span>
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" sideOffset={8} className="w-auto rounded-xl p-3">
        <div
          role="slider"
          tabIndex={0}
          aria-label={t('config:interfaceSettings.colorSpectrum')}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(hsv.saturation * 100)}
          aria-valuetext={draft}
          aria-describedby={hintId}
          className="relative h-34 w-34 touch-none rounded-lg outline-none focus-visible:ring-2 focus-visible:ring-ring"
          style={{
            background: `linear-gradient(to top, #000, transparent), linear-gradient(to right, #fff, transparent), hsl(${hsv.hue} 100% 50%)`,
          }}
          onPointerDown={(event) => {
            event.currentTarget.setPointerCapture(event.pointerId)
            point(event)
          }}
          onPointerMove={(event) => {
            if (event.currentTarget.hasPointerCapture(event.pointerId)) point(event)
          }}
          onPointerUp={(event) => {
            if (!event.currentTarget.hasPointerCapture(event.pointerId)) return
            point(event)
            event.currentTarget.releasePointerCapture(event.pointerId)
            commit()
          }}
          onPointerCancel={() => {
            changeDraft(value)
            setHsv(colorHsv(value))
          }}
          onKeyDown={(event) => {
            if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(event.key)) return
            event.preventDefault()
            changeColor({
              ...hsv,
              saturation: Math.min(
                1,
                Math.max(
                  0,
                  hsv.saturation +
                    (event.key === 'ArrowRight' ? 0.01 : event.key === 'ArrowLeft' ? -0.01 : 0),
                ),
              ),
              brightness: Math.min(
                1,
                Math.max(
                  0,
                  hsv.brightness +
                    (event.key === 'ArrowUp' ? 0.01 : event.key === 'ArrowDown' ? -0.01 : 0),
                ),
              ),
            })
          }}
          onKeyUp={(event) => {
            if (event.key.startsWith('Arrow')) commit()
          }}
          onBlur={commit}
        >
          <span
            aria-hidden="true"
            className="pointer-events-none absolute size-3 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-sm"
            style={{ left: `${hsv.saturation * 100}%`, top: `${(1 - hsv.brightness) * 100}%` }}
          />
        </div>
        <input
          type="range"
          min={0}
          max={359}
          step={1}
          value={Math.round(hsv.hue)}
          aria-label={t('config:interfaceSettings.colorHue')}
          className="w-34"
          onChange={(event) => changeColor({ ...hsv, hue: Number(event.target.value) })}
          onPointerUp={commit}
          onBlur={commit}
          onKeyUp={(event) => {
            if (event.key.startsWith('Arrow') || event.key === 'Home' || event.key === 'End')
              commit()
          }}
        />
        <input
          type="text"
          value={draft}
          spellCheck={false}
          aria-label={t('config:interfaceSettings.customAccentHex')}
          aria-invalid={invalid}
          aria-describedby={hintId}
          className={cn(
            'h-7 w-34 rounded-full border bg-background px-2 font-mono text-xs',
            invalid && 'border-destructive',
          )}
          onChange={(event) => {
            changeDraft(event.target.value)
            const hex = normalizeHexColor(event.target.value)
            if (hex) setHsv(colorHsv(hex))
          }}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault()
              commit()
            }
          }}
        />
        <p id={hintId} className="max-w-34 text-xs text-muted-foreground">
          {invalid
            ? t('config:interfaceSettings.invalidAccentColor')
            : t('config:interfaceSettings.colorSpectrumHint')}
        </p>
      </PopoverContent>
    </Popover>
  )
}

export function InterfaceSettings({
  notify,
  defaultAdvancedExpanded = false,
  navigationKey,
}: {
  notify: Notify
  defaultAdvancedExpanded?: boolean
  navigationKey?: number
}) {
  const { t } = useI18n()
  const {
    theme,
    density,
    accent,
    customAccent,
    uiFontSize,
    codeFontSize,
    radius,
    motion,
    setTheme,
    setDensity,
    setAccent,
    setCustomAccent,
    setUiFontSize,
    setCodeFontSize,
    setRadius,
    setMotion,
    resetAppearance,
  } = useUiStore()
  const [advancedExpanded, setAdvancedExpanded] = useState(defaultAdvancedExpanded)
  const advancedId = useId()
  const themeName = useId()
  useEffect(() => {
    if (defaultAdvancedExpanded) setAdvancedExpanded(true)
  }, [defaultAdvancedExpanded, navigationKey])
  const changed =
    theme !== DEFAULT_UI_PREFERENCES.theme ||
    density !== DEFAULT_UI_PREFERENCES.density ||
    accent !== DEFAULT_UI_PREFERENCES.accent ||
    customAccent !== DEFAULT_UI_PREFERENCES.customAccent ||
    uiFontSize !== DEFAULT_UI_PREFERENCES.uiFontSize ||
    codeFontSize !== DEFAULT_UI_PREFERENCES.codeFontSize ||
    radius !== DEFAULT_UI_PREFERENCES.radius ||
    motion !== DEFAULT_UI_PREFERENCES.motion
  const update =
    <T,>(setter: (value: T) => void) =>
    (value: T) => {
      setter(value)
      notify(t('config:interfaceSettings.preferencesUpdated'))
    }
  const themeOptions: Choice<ThemeMode>[] = [
    {
      value: 'system',
      label: t('config:interfaceSettings.followSystem'),
      description: t('config:interfaceSettings.followSystemDescription'),
      icon: MonitorCog,
    },
    {
      value: 'light',
      label: t('config:interfaceSettings.light'),
      description: t('config:interfaceSettings.lightDescription'),
      icon: Sun,
    },
    {
      value: 'dark',
      label: t('config:interfaceSettings.dark'),
      description: t('config:interfaceSettings.darkDescription'),
      icon: Moon,
    },
  ]

  const densityOptions: Choice<DensityMode>[] = [
    {
      value: 'comfortable',
      label: t('config:interfaceSettings.comfortable'),
      description: t('config:interfaceSettings.comfortableDescription'),
      icon: UnfoldVertical,
    },
    {
      value: 'compact',
      label: t('config:interfaceSettings.compact'),
      description: t('config:interfaceSettings.compactDescription'),
      icon: FoldVertical,
    },
  ]

  const motionOptions: Choice<MotionMode>[] = [
    {
      value: 'system',
      label: t('config:interfaceSettings.motionSystem'),
      description: t('config:interfaceSettings.motionSystemDescription'),
      icon: MonitorCog,
    },
    {
      value: 'full',
      label: t('config:interfaceSettings.motionFull'),
      description: t('config:interfaceSettings.motionFullDescription'),
      icon: Sparkles,
    },
    {
      value: 'reduced',
      label: t('config:interfaceSettings.motionReduced'),
      description: t('config:interfaceSettings.motionReducedDescription'),
      icon: Activity,
    },
  ]

  const accentOptions: Array<{ value: AccentPreset; label: string }> = [
    { value: 'neutral', label: t('config:interfaceSettings.accentNeutral') },
    { value: 'blue', label: t('config:interfaceSettings.accentBlue') },
    { value: 'teal', label: t('config:interfaceSettings.accentTeal') },
    { value: 'violet', label: t('config:interfaceSettings.accentViolet') },
    { value: 'coral', label: t('config:interfaceSettings.accentCoral') },
    { value: 'custom', label: t('config:interfaceSettings.accentCustom') },
  ]

  const radiusOptions: Array<{ value: RadiusMode; label: string }> = [
    { value: 'sharp', label: t('config:interfaceSettings.sharp') },
    { value: 'default', label: t('config:interfaceSettings.standard') },
    { value: 'soft', label: t('config:interfaceSettings.soft') },
  ]

  return (
    <div className="flex flex-col gap-6">
      <SettingsSection
        title={t('config:interfaceSettings.visualStyle')}
        description={t('config:interfaceSettings.appearanceDescription')}
        data-config-card="interface-appearance"
        action={
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={!changed}
            onClick={() => {
              resetAppearance()
              notify(t('config:interfaceSettings.defaultsRestored'))
            }}
          >
            <RotateCcw size={14} />
            {t('config:interfaceSettings.restoreDefaults')}
          </Button>
        }
      >
        <SettingsRow
          title={t('config:interfaceSettings.colorMode')}
          control={
            <div
              role="radiogroup"
              aria-label={t('config:interfaceSettings.colorMode')}
              className="grid w-68 max-w-full grid-cols-3 gap-4"
            >
              {themeOptions.map((option) => (
                <label
                  key={option.value}
                  className="group flex min-w-0 cursor-pointer flex-col items-center gap-2 text-xs text-muted-foreground"
                >
                  <input
                    className="peer sr-only"
                    type="radio"
                    name={themeName}
                    value={option.value}
                    checked={theme === option.value}
                    onChange={() => update(setTheme)(option.value)}
                  />
                  <span
                    aria-hidden="true"
                    className={cn(
                      'relative block aspect-4/3 w-full overflow-hidden rounded-lg border border-border bg-[#f0f0f0] peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-ring',
                      theme === option.value && 'ring-2 ring-ring',
                    )}
                  >
                    <span className="absolute inset-2 rounded border border-black/10 bg-white">
                      <span className="absolute inset-y-0 left-0 w-1/4 border-r border-black/10 bg-[#f5f5f5]" />
                      <span className="absolute top-3 right-1 h-0.5 w-1/2 rounded bg-black/15" />
                      <span className="absolute top-5 right-2 h-0.5 w-1/3 rounded bg-black/15" />
                    </span>
                    {option.value !== 'light' && (
                      <span
                        className={cn(
                          'absolute inset-2 rounded bg-[#181818]',
                          option.value === 'system' && 'left-1/2 rounded-l-none',
                        )}
                      >
                        <span className="absolute top-3 right-1 h-0.5 w-1/2 rounded bg-white/25" />
                        <span className="absolute top-5 right-2 h-0.5 w-1/3 rounded bg-white/25" />
                      </span>
                    )}
                  </span>
                  <span className={cn(theme === option.value && 'text-foreground')}>
                    {option.label}
                  </span>
                </label>
              ))}
            </div>
          }
        />
        <SettingsRow
          title={t('config:interfaceSettings.accentColor')}
          description={t('config:interfaceSettings.accentDescription')}
          control={
            <div className="flex flex-wrap items-center justify-end gap-2">
              <Select
                value={accent}
                onValueChange={(next) => {
                  const option = accentOptions.find((item) => item.value === next)
                  if (option) update(setAccent)(option.value)
                }}
              >
                <SelectTrigger
                  aria-label={t('config:interfaceSettings.accentColor')}
                  className="rounded-full"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent position="popper" align="end">
                  {accentOptions.map((option) => (
                    <SelectItem key={option.value} value={option.value}>
                      <span
                        aria-hidden="true"
                        className="size-3 rounded-full border border-black/10"
                        style={{
                          backgroundColor:
                            option.value === 'custom'
                              ? customAccent
                              : ACCENT_SWATCHES[option.value],
                        }}
                      />
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {accent === 'custom' && (
                <AccentColorPicker
                  value={normalizeHexColor(customAccent) || DEFAULT_UI_PREFERENCES.customAccent}
                  onCommit={update(setCustomAccent)}
                />
              )}
            </div>
          }
        />
      </SettingsSection>
      <Collapsible open={advancedExpanded} onOpenChange={setAdvancedExpanded}>
        <CollapsibleTrigger asChild>
          <button
            type="button"
            aria-controls={advancedId}
            aria-expanded={advancedExpanded}
            className="flex min-h-11 w-full items-center justify-between rounded-lg text-sm font-medium text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
          >
            {t('config:interfaceSettings.advanced')}
            <ChevronDown
              size={16}
              aria-hidden="true"
              className={cn('transition-transform', advancedExpanded && 'rotate-180')}
            />
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent id={advancedId}>
          <div className="divide-y divide-border" data-config-card="interface-style">
            <SettingsRow
              title={t('config:interfaceSettings.uiFontSize')}
              description={t('config:interfaceSettings.numericFontHint')}
              control={
                <TypographySizeInput
                  role="ui"
                  label={t('config:interfaceSettings.uiFontSize')}
                  value={uiFontSize}
                  onCommit={update(setUiFontSize)}
                />
              }
            />
            <SettingsRow
              title={t('config:interfaceSettings.codeFontSize')}
              control={
                <TypographySizeInput
                  role="code"
                  label={t('config:interfaceSettings.codeFontSize')}
                  value={codeFontSize}
                  onCommit={update(setCodeFontSize)}
                />
              }
            />
            <SettingsRow
              title={t('config:interfaceSettings.cornerStyle')}
              control={
                <CompactChoices
                  label={t('config:interfaceSettings.cornerStyle')}
                  options={radiusOptions}
                  value={radius}
                  onChange={update(setRadius)}
                />
              }
            />
          </div>
          <div className="divide-y divide-border" data-config-card="interface-motion">
            <SettingsRow
              title={t('config:interfaceSettings.interfaceDensity')}
              control={
                <CompactChoices
                  label={t('config:interfaceSettings.interfaceDensity')}
                  options={densityOptions}
                  value={density}
                  onChange={update(setDensity)}
                />
              }
            />
            <SettingsRow
              title={t('config:interfaceSettings.motion')}
              control={
                <CompactChoices
                  label={t('config:interfaceSettings.motion')}
                  options={motionOptions}
                  value={motion}
                  onChange={update(setMotion)}
                />
              }
            />
            <SettingsRow
              title={t('config:interfaceSettings.automaticByTime')}
              description={t('config:interfaceSettings.automaticByTimeDescription')}
              control={
                <Switch
                  checked={theme === 'scheduled'}
                  onCheckedChange={(checked) => update(setTheme)(checked ? 'scheduled' : 'system')}
                  aria-label={t('config:interfaceSettings.automaticByTime')}
                />
              }
            />
          </div>
        </CollapsibleContent>
      </Collapsible>
      <LanguageSettings notify={notify} />
    </div>
  )
}
