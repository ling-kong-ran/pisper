// 界面客制化入口:预设受设计 Token 约束;自定义强调色由即时派生的变量规则保证可读性。
import { useEffect, useState } from 'react'
import {
  Activity,
  Check,
  ChevronDown,
  Clock3,
  FoldVertical,
  MonitorCog,
  Moon,
  Palette,
  RotateCcw,
  Sparkles,
  Sun,
  Type,
  UnfoldVertical,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { normalizeHexColor } from '@/lib/custom-accent'
import { LanguageSettings } from './LanguageSettings'
import { SettingsCard as Panel } from './settings-primitives'
import { Button } from '@/components/ui/button'
import { RadioGroup, RadioGroupOption } from '@/components/ui/radio-group'
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
  type FontScale,
  type MotionMode,
  type RadiusMode,
  type ThemeMode,
} from '@/stores/ui-store'
import type { Notify } from '@/app/route-context'
import type { LucideIcon } from 'lucide-react'
import { cn } from '@/lib/utils'

type Choice<T extends string> = {
  value: T
  label: string
  description: string
  icon: LucideIcon
}

function SettingHeading({ title, description }: { title: string; description: string }) {
  return (
    <div className="flex items-start gap-[11px] [&_h2]:text-[16px] [&_p]:mt-1 [&_p]:text-[13px] [&_p]:leading-[1.55] [&_p]:text-[var(--text-muted)]">
      <div>
        <h2>{title}</h2>
        <p>{description}</p>
      </div>
    </div>
  )
}

function ChoiceGrid<T extends string>({
  label,
  options,
  value,
  onChange,
  className,
  preview = false,
}: {
  label: string
  options: Choice<T>[]
  value: T
  onChange: (value: T) => void
  className?: string
  preview?: boolean
}) {
  return (
    <RadioGroup
      value={value}
      onValueChange={(next) => {
        const option = options.find((item) => item.value === next)
        if (option) onChange(option.value)
      }}
      className={cn(
        preview
          ? 'grid w-full grid-cols-2 gap-3 min-[651px]:flex min-[651px]:w-auto min-[651px]:flex-wrap min-[651px]:justify-end'
          : 'mt-4 grid grid-cols-4 gap-2 max-[650px]:grid-cols-1 max-[1300px]:grid-cols-2',
        className,
      )}
      aria-label={label}
    >
      {options.map((option) => {
        const selected = option.value === value
        const Icon = option.icon
        if (preview) {
          return (
            <RadioGroupOption
              key={option.value}
              type="button"
              value={option.value}
              aria-label={`${option.label} ${option.description}`}
              title={option.description}
              className={cn(
                'group flex w-full min-w-0 flex-col items-center gap-1.5 rounded-xl p-1 text-xs text-muted-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring min-[651px]:w-20',
                selected && 'text-foreground',
              )}
            >
              <span
                aria-hidden="true"
                className={cn(
                  'relative block h-14 w-full overflow-hidden rounded-lg border-2 border-border bg-[#f0f0f0] p-1 transition group-hover:border-ring/50',
                  selected && 'border-ring',
                )}
              >
                <span className="absolute inset-1 rounded border border-black/10 bg-white shadow-sm">
                  <span className="absolute inset-y-0 left-0 w-3 border-r border-black/10 bg-[#f5f5f5]" />
                  <span className="absolute top-2 right-1 h-0.5 w-6 rounded bg-ring" />
                  <span className="absolute top-4 left-4 h-0.5 w-7 rounded bg-black/15" />
                  <span className="absolute top-6 left-4 h-0.5 w-5 rounded bg-black/15" />
                  <span className="absolute right-1 bottom-1 left-4 h-2 rounded-full border border-black/10" />
                </span>
                {option.value !== 'light' && (
                  <span
                    className={cn(
                      'absolute inset-1 overflow-hidden rounded bg-[#0f0f0f]',
                      option.value !== 'dark' && 'left-1/2 rounded-l-none',
                    )}
                  >
                    <span className="absolute top-2 right-1 h-0.5 w-6 rounded bg-ring" />
                    <span className="absolute top-4 right-1 h-0.5 w-7 rounded bg-white/25" />
                    <span className="absolute top-6 right-3 h-0.5 w-5 rounded bg-white/25" />
                    <span className="absolute right-1 bottom-1 left-1 h-2 rounded-full border border-white/25" />
                  </span>
                )}
                {option.value === 'scheduled' && (
                  <Clock3 className="absolute top-1 left-1 size-3 text-black" />
                )}
              </span>
              <span>{option.label}</span>
            </RadioGroupOption>
          )
        }
        return (
          <RadioGroupOption
            type="button"
            className={`grid min-h-[72px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 rounded-[var(--r-sm)] border p-2.5 text-left transition hover:border-[var(--star)] hover:bg-[var(--accent-soft)] ${selected ? 'border-[var(--star)] bg-[var(--star-soft)] ' : 'border-[var(--stroke)] bg-[var(--surface-subtle)]'}`}
            value={option.value}
            key={option.value}
          >
            <span className="grid size-8 place-items-center rounded-[var(--r-xs)] bg-[var(--solid)] text-[var(--star-strong)]">
              <Icon size={16} />
            </span>
            <span className="flex min-w-0 flex-col gap-0.5">
              <strong className="text-[13px]">{option.label}</strong>
              <small className="text-[var(--text-muted)]">{option.description}</small>
            </span>
            {selected && (
              <span className="grid size-5 place-items-center rounded-full bg-[var(--star)] text-[var(--on-accent)]">
                <Check size={13} />
              </span>
            )}
          </RadioGroupOption>
        )
      })}
    </RadioGroup>
  )
}

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
          className={`min-h-9 max-[650px]:min-h-11 whitespace-nowrap rounded-lg px-2 text-sm font-normal transition ${option.value === value ? 'bg-[var(--solid)] text-[var(--text)] shadow-sm' : 'text-[var(--text-muted)] hover:text-[var(--text)]'}`}
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
  custom: '#1677e8', // 仅兜底,渲染时以 customAccent 为准
}

export function InterfaceSettings({ notify }: { notify: Notify }) {
  const { t } = useI18n()
  const theme = useUiStore((state) => state.theme)
  const density = useUiStore((state) => state.density)
  const accent = useUiStore((state) => state.accent)
  const customAccent = useUiStore((state) => state.customAccent)
  const fontScale = useUiStore((state) => state.fontScale)
  const radius = useUiStore((state) => state.radius)
  const motion = useUiStore((state) => state.motion)
  const setTheme = useUiStore((state) => state.setTheme)
  const setDensity = useUiStore((state) => state.setDensity)
  const setAccent = useUiStore((state) => state.setAccent)
  const setCustomAccent = useUiStore((state) => state.setCustomAccent)
  const setFontScale = useUiStore((state) => state.setFontScale)
  const setRadius = useUiStore((state) => state.setRadius)
  const setMotion = useUiStore((state) => state.setMotion)
  const resetAppearance = useUiStore((state) => state.resetAppearance)

  const changed =
    theme !== DEFAULT_UI_PREFERENCES.theme ||
    density !== DEFAULT_UI_PREFERENCES.density ||
    accent !== DEFAULT_UI_PREFERENCES.accent ||
    customAccent !== DEFAULT_UI_PREFERENCES.customAccent ||
    fontScale !== DEFAULT_UI_PREFERENCES.fontScale ||
    radius !== DEFAULT_UI_PREFERENCES.radius ||
    motion !== DEFAULT_UI_PREFERENCES.motion

  // HEX 草稿:允许中间态输入,只有完整 6 位才落库生效
  const [hexDraft, setHexDraft] = useState(customAccent)
  const [advancedOpen, setAdvancedOpen] = useState(true)
  useEffect(() => setHexDraft(customAccent), [customAccent])
  const normalizedDraft = normalizeHexColor(hexDraft)
  const hexInvalid = Boolean(hexDraft.trim()) && !normalizedDraft
  const commitHex = (value: string) => {
    setHexDraft(value)
    const normalized = normalizeHexColor(value)
    if (normalized) update(setCustomAccent)(normalized)
  }

  const announceChange = () => notify(t('config:interfaceSettings.preferencesUpdated'))
  const update =
    <T,>(setter: (value: T) => void) =>
    (value: T) => {
      setter(value)
      announceChange()
    }

  const themeOptions: Choice<ThemeMode>[] = [
    {
      value: 'system',
      label: t('config:interfaceSettings.followSystem'),
      description: t('config:interfaceSettings.followSystemDescription'),
      icon: MonitorCog,
    },
    {
      value: 'scheduled',
      label: t('config:interfaceSettings.automaticByTime'),
      description: t('config:interfaceSettings.automaticByTimeDescription'),
      icon: Clock3,
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

  const fontOptions: Array<{ value: FontScale; label: string }> = [
    { value: 'small', label: t('config:interfaceSettings.small') },
    { value: 'default', label: t('config:interfaceSettings.standard') },
    { value: 'large', label: t('config:interfaceSettings.large') },
    { value: 'xlarge', label: t('config:interfaceSettings.extraLarge') },
  ]
  const radiusOptions: Array<{ value: RadiusMode; label: string }> = [
    { value: 'sharp', label: t('config:interfaceSettings.sharp') },
    { value: 'default', label: t('config:interfaceSettings.standard') },
    { value: 'soft', label: t('config:interfaceSettings.soft') },
  ]

  return (
    <div className="flex flex-col gap-6">
      <Panel
        className="border-0 bg-transparent p-0 shadow-none ring-0"
        data-config-card="interface-appearance"
      >
        <div className="flex items-start justify-between gap-4 max-[650px]:flex-col">
          <SettingHeading
            title={t('config:interfaceSettings.visualStyle')}
            description={t('config:interfaceSettings.appearanceDescription')}
          />
          <Button
            type="button"
            variant="outline"
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
        </div>

        <div className="mt-5 flex flex-wrap items-center justify-between gap-4 rounded-2xl border border-border p-4">
          <span className="text-sm font-medium">{t('config:interfaceSettings.colorMode')}</span>
          <ChoiceGrid
            label={t('config:interfaceSettings.colorMode')}
            options={themeOptions}
            value={theme}
            onChange={update(setTheme)}
            preview
          />
        </div>

        <div className="mt-4 rounded-2xl border border-border p-4">
          <div className="flex flex-wrap items-center justify-between gap-4">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 text-sm font-medium text-foreground">
                <Palette size={15} />
                {t('config:interfaceSettings.accentColor')}
              </div>
              <p className="mt-1 text-xs text-content-muted">
                {t('config:interfaceSettings.accentDescription')}
              </p>
            </div>
            <div className="flex shrink-0 flex-wrap items-center gap-3">
              <Select
                value={accent}
                onValueChange={(next) => {
                  const option = accentOptions.find((item) => item.value === next)
                  if (option) update(setAccent)(option.value)
                }}
              >
                <SelectTrigger
                  aria-label={t('config:interfaceSettings.accentColor')}
                  className="min-h-9 rounded-full max-[650px]:min-h-11"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent position="popper" className="rounded-xl">
                  {accentOptions.map((option) => (
                    <SelectItem
                      key={option.value}
                      value={option.value}
                      className="min-h-9 max-[650px]:min-h-11"
                    >
                      <span
                        aria-hidden="true"
                        className="size-3.5 rounded-full border border-black/10"
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
            </div>
          </div>
          {accent === 'custom' && (
            <div className="mt-3 flex flex-wrap items-center justify-end gap-3">
              <input
                type="color"
                value={normalizeHexColor(customAccent) || '#1677e8'}
                onChange={(event) => update(setCustomAccent)(event.target.value)}
                aria-label={t('config:interfaceSettings.customAccentColor')}
                className="h-9 w-14 cursor-pointer rounded-[var(--r-xs)] border border-[var(--stroke)] bg-[var(--solid)] p-1"
              />
              <input
                type="text"
                value={hexDraft}
                onChange={(event) => commitHex(event.target.value)}
                spellCheck={false}
                aria-invalid={hexInvalid}
                aria-describedby="custom-accent-hint"
                aria-label={t('config:interfaceSettings.customAccentHex')}
                className={`h-9 w-[116px] rounded-[var(--r-xs)] border bg-[var(--solid)] px-2.5 font-mono text-[12.5px] text-[var(--text)] ${hexInvalid ? 'border-[var(--danger)]' : 'border-[var(--stroke)]'}`}
              />
              <span
                id="custom-accent-hint"
                className={`text-[12px] ${hexInvalid ? 'text-[var(--danger)]' : 'text-[var(--text-muted)]'}`}
              >
                {hexInvalid
                  ? t('config:interfaceSettings.invalidAccentColor')
                  : t('config:interfaceSettings.customAccentHint')}
              </span>
            </div>
          )}
        </div>
      </Panel>

      <details
        className="group/advanced"
        open={advancedOpen}
        onToggle={(event) => setAdvancedOpen(event.currentTarget.open)}
      >
        <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between rounded-lg px-1 text-sm font-medium text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
          {t('config:interfaceSettings.advanced')}
          <ChevronDown
            aria-hidden="true"
            size={16}
            className="transition-transform group-open/advanced:rotate-180"
          />
        </summary>
        <div className="mt-3 flex flex-col gap-4">
          <div
            data-config-card="interface-style"
            className="grid grid-cols-[minmax(0,1fr)_minmax(240px,.72fr)] gap-4 rounded-2xl border border-border p-4 max-[900px]:grid-cols-1"
          >
            <div>
              <div className="flex items-center gap-2 text-sm font-medium text-foreground">
                <Type size={15} />
                {t('config:interfaceSettings.interfaceStyle')}
              </div>
              <div className="mt-3 grid grid-cols-2 gap-3 max-[470px]:grid-cols-1">
                <div className="grid content-start gap-1.5">
                  <span className="text-[12px] text-[var(--text-muted)]">
                    {t('config:interfaceSettings.fontSize')}
                  </span>
                  <CompactChoices
                    label={t('config:interfaceSettings.fontSize')}
                    options={fontOptions}
                    value={fontScale}
                    onChange={update(setFontScale)}
                  />
                  <p className="text-xs leading-relaxed text-muted-foreground">
                    {t('config:interfaceSettings.fontScaleHint')}
                  </p>
                </div>
                <div className="grid content-start gap-1.5">
                  <span className="text-[12px] text-[var(--text-muted)]">
                    {t('config:interfaceSettings.cornerStyle')}
                  </span>
                  <CompactChoices
                    label={t('config:interfaceSettings.cornerStyle')}
                    options={radiusOptions}
                    value={radius}
                    onChange={update(setRadius)}
                  />
                </div>
              </div>
            </div>

            <div
              className="overflow-hidden rounded-[var(--r-md)] border border-[var(--stroke)] bg-[var(--main-surface-bg)] shadow-[var(--sh-surface)]"
              aria-label={t('config:interfaceSettings.livePreview')}
            >
              <div className="flex h-8 items-center gap-1.5 border-b border-[var(--stroke)] bg-[var(--surface-subtle)] px-3">
                <span className="size-1.5 rounded-full bg-[var(--brand-blue)]" />
                <span className="text-[11px] font-bold text-[var(--text-muted)]">
                  {t('config:interfaceSettings.livePreview')}
                </span>
              </div>
              <div className="grid gap-2.5 p-3">
                <div className="mr-7 rounded-[var(--r-sm)] bg-transparent p-2 text-[var(--app-font-size)] text-[var(--text)]">
                  {t('config:interfaceSettings.previewAgentMessage')}
                </div>
                <div className="ml-7 rounded-[var(--r-sm)] bg-[var(--user-bubble-bg)] p-2 text-[var(--app-font-size)] text-[var(--user-bubble-text)]">
                  {t('config:interfaceSettings.previewUserMessage')}
                </div>
                <div className="flex items-center gap-2">
                  <span className="h-1.5 flex-1 overflow-hidden rounded-full bg-[var(--surface-muted)]">
                    <span className="block h-full w-2/3 rounded-full bg-[var(--brand-blue)]" />
                  </span>
                  <span className="text-[11px] text-[var(--text-muted)]">67%</span>
                </div>
              </div>
            </div>
          </div>
          <Panel className="rounded-2xl p-4 shadow-none" data-config-card="interface-motion">
            <SettingHeading
              title={t('config:interfaceSettings.displayAndMotion')}
              description={t('config:interfaceSettings.displayAndMotionDescription')}
            />
            <div className="mt-4 flex flex-col divide-y divide-border">
              <div className="flex flex-wrap items-center justify-between gap-3 py-3">
                <div className="text-sm font-medium">
                  {t('config:interfaceSettings.interfaceDensity')}
                </div>
                <CompactChoices
                  label={t('config:interfaceSettings.interfaceDensity')}
                  options={densityOptions}
                  value={density}
                  onChange={update(setDensity)}
                />
              </div>
              <div className="flex flex-wrap items-center justify-between gap-3 py-3">
                <div className="text-sm font-medium">{t('config:interfaceSettings.motion')}</div>
                <CompactChoices
                  label={t('config:interfaceSettings.motion')}
                  options={motionOptions}
                  value={motion}
                  onChange={update(setMotion)}
                />
              </div>
            </div>
          </Panel>
        </div>
      </details>

      <LanguageSettings notify={notify} />
    </div>
  )
}
