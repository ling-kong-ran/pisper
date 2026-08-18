// 语言设置：zh-CN / en-US 切换，即时生效并同步桌面壳。
import { Check, FoldVertical, Languages, UnfoldVertical } from 'lucide-react'
import { LANGUAGE_OPTIONS, translateText, useI18n } from '@/app/use-i18n'
import { SettingsCard as Panel } from './settings-primitives'
import { useUiStore, type DensityMode } from '@/stores/ui-store'
import type { LucideIcon } from 'lucide-react'
import type { SupportedLanguage } from '@/app/i18n'
import type { Notify } from '@/app/route-context'

import { AppNotice } from '@/components/ui/app-primitives'

function languageName(language: SupportedLanguage, t: ReturnType<typeof useI18n>['t']) {
  return language === 'en-US'
    ? t('config:languageSettings.english')
    : t('config:languageSettings.simplifiedChinese')
}

export function LanguageSettings({ notify }: { notify: Notify }) {
  const { language, setLanguage, t } = useI18n()
  const density = useUiStore((state) => state.density)
  const setDensity = useUiStore((state) => state.setDensity)

  const selectLanguage = (nextLanguage: SupportedLanguage) => {
    if (nextLanguage === language) return
    setLanguage(nextLanguage)
    notify(
      nextLanguage === 'en-US'
        ? translateText('config:languageSettings.displayLanguageChangedToEnglish', nextLanguage)
        : translateText(
            'config:languageSettings.displayLanguageChangedToSimplifiedChinese',
            nextLanguage,
          ),
    )
  }

  const selectDensity = (nextDensity: DensityMode) => {
    if (nextDensity === density) return
    setDensity(nextDensity)
    notify(
      t('config:languageSettings.interfaceDensitySwitchedToDensity', {
        density:
          nextDensity === 'compact'
            ? t('config:languageSettings.compact')
            : t('config:languageSettings.comfortable'),
      }),
    )
  }

  const densityOptions: Array<[DensityMode, string, string, LucideIcon]> = [
    [
      'comfortable',
      t('config:languageSettings.comfortable'),
      t('config:languageSettings.defaultSpacingAndSizing'),
      UnfoldVertical,
    ],
    [
      'compact',
      t('config:languageSettings.compact'),
      t('config:languageSettings.fitMoreContentOnScreen'),
      FoldVertical,
    ],
  ]

  return (
    <div className="flex w-[min(100%,_760px)] flex-col gap-[12px]">
      <Panel className="[padding:18px]">
        <div className="language-settings-heading flex items-start gap-[11px] [&_h2]:text-[16px] [&_p]:mt-[4px] [&_p]:text-[var(--text-muted)] [&_p]:text-[13px] [&_p]:leading-[1.55]">
          <span className="grid w-[38px] h-[38px] [flex:0_0_auto] place-items-center rounded-[11px] bg-[var(--star-soft)] text-[var(--star-strong)]">
            <Languages size={19} />
          </span>
          <div>
            <h2>{t('config:languageSettings.displayLanguage')}</h2>
            <p>
              {t(
                'config:languageSettings.chooseTheLanguageUsedByPisperChangesApplyImmediatelyAndAreSavedForYourNextVisit',
              )}
            </p>
          </div>
        </div>
        <div
          className="language-choice-grid max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(2,_minmax(0,_1fr))] gap-[9px] [margin-top:18px]"
          role="radiogroup"
          aria-label={t('config:languageSettings.displayLanguage')}
        >
          {LANGUAGE_OPTIONS.map((option) => {
            const selected = option.value === language
            return (
              <button
                type="button"
                className={`language-choice hover:border-[var(--star)] hover:bg-[var(--accent-soft)] hover:[transform:translateY(-1px)] [&.selected]:border-[var(--star)] [&.selected]:bg-[var(--star-soft)] [&.selected]:shadow-[0_0_0_3px_var(--accent-ring)] grid min-h-[80px] grid-cols-[auto_minmax(0,_1fr)_auto] items-center gap-[10px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:11px] text-[var(--text)] text-left [transition:border-color_var(--d1)_var(--ease-out),_background_var(--d1)_var(--ease-out),_box-shadow_var(--d1)_var(--ease-out),_transform_var(--d1)_var(--ease-out)] ${selected ? 'selected' : ''}`}
                role="radio"
                aria-checked={selected}
                onClick={() => selectLanguage(option.value)}
                key={option.value}
              >
                <span
                  className={`language-choice-mark grid w-[34px] h-[34px] place-items-center rounded-[9px] bg-[var(--solid)] text-[var(--star-strong)] text-[12px] font-[800] tracking-[.03em] ${option.value === 'en-US' ? 'english [.language-choice-mark&]:bg-[var(--brand-blue-soft)] [.language-choice-mark&]:text-[var(--brand-blue-strong)]' : ''}`}
                >
                  {option.shortName}
                </span>
                <span className="language-choice-copy [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] flex min-w-0 flex-col gap-[3px]">
                  <strong>{languageName(option.value, t)}</strong>
                  <small>
                    {option.value === 'zh-CN'
                      ? t('config:languageSettings.chineseInterface')
                      : t('config:languageSettings.englishInterface')}
                  </small>
                </span>
                {selected && (
                  <span className="grid w-[21px] h-[21px] place-items-center rounded-[50%] bg-[var(--star)] text-[var(--on-accent)]">
                    <Check size={15} />
                  </span>
                )}
              </button>
            )
          })}
        </div>
        <AppNotice className="[margin-top:15px]">
          <Languages size={16} />
          <span>
            <strong>
              {t('config:languageSettings.currentLanguage')} · {languageName(language, t)}
            </strong>
            <small>
              {t(
                'config:languageSettings.thisSettingChangesInterfaceTextOnlyItDoesNotControlTheLanguageOfAgentResponses',
              )}
            </small>
          </span>
        </AppNotice>
        <small className="block [margin:9px_1px_0] text-[var(--text-muted)] text-[12px]">
          {t('config:languageSettings.languagePreferenceIsStoredInThisBrowser')}
        </small>
      </Panel>
      <Panel className="[padding:18px] density-settings-card">
        <div className="language-settings-heading flex items-start gap-[11px] [&_h2]:text-[16px] [&_p]:mt-[4px] [&_p]:text-[var(--text-muted)] [&_p]:text-[13px] [&_p]:leading-[1.55]">
          <span className="grid w-[38px] h-[38px] [flex:0_0_auto] place-items-center rounded-[11px] bg-[var(--star-soft)] text-[var(--star-strong)]">
            <FoldVertical size={19} />
          </span>
          <div>
            <h2>{t('config:languageSettings.interfaceDensity')}</h2>
            <p>
              {t(
                'config:languageSettings.adjustSpacingAndSizingCompactModeFitsMoreContentOnScreen',
              )}
            </p>
          </div>
        </div>
        <div
          className="language-choice-grid max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(2,_minmax(0,_1fr))] gap-[9px] [margin-top:18px]"
          role="radiogroup"
          aria-label={t('config:languageSettings.interfaceDensity')}
        >
          {densityOptions.map(([value, label, description, Icon]) => {
            const selected = value === density
            return (
              <button
                type="button"
                className={`language-choice hover:border-[var(--star)] hover:bg-[var(--accent-soft)] hover:[transform:translateY(-1px)] [&.selected]:border-[var(--star)] [&.selected]:bg-[var(--star-soft)] [&.selected]:shadow-[0_0_0_3px_var(--accent-ring)] grid min-h-[80px] grid-cols-[auto_minmax(0,_1fr)_auto] items-center gap-[10px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:11px] text-[var(--text)] text-left [transition:border-color_var(--d1)_var(--ease-out),_background_var(--d1)_var(--ease-out),_box-shadow_var(--d1)_var(--ease-out),_transform_var(--d1)_var(--ease-out)] ${selected ? 'selected' : ''}`}
                role="radio"
                aria-checked={selected}
                onClick={() => selectDensity(value)}
                key={value}
              >
                <span className="language-choice-mark grid w-[34px] h-[34px] place-items-center rounded-[9px] bg-[var(--solid)] text-[var(--star-strong)] text-[12px] font-[800] tracking-[.03em]">
                  <Icon size={16} />
                </span>
                <span className="language-choice-copy [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] flex min-w-0 flex-col gap-[3px]">
                  <strong>{label}</strong>
                  <small>{description}</small>
                </span>
                {selected && (
                  <span className="grid w-[21px] h-[21px] place-items-center rounded-[50%] bg-[var(--star)] text-[var(--on-accent)]">
                    <Check size={15} />
                  </span>
                )}
              </button>
            )
          })}
        </div>
        <AppNotice className="[margin-top:15px]">
          <FoldVertical size={16} />
          <span>
            <strong>
              {t('config:languageSettings.currentDensity')} ·{' '}
              {density === 'compact'
                ? t('config:languageSettings.compact')
                : t('config:languageSettings.comfortable')}
            </strong>
            <small>
              {t('config:languageSettings.thisSettingOnlyAffectsTheInterfaceInThisBrowser')}
            </small>
          </span>
        </AppNotice>
      </Panel>
    </div>
  )
}
