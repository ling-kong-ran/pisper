// 语言设置独立成卡片，避免与外观偏好耦合；切换后即时同步应用语言。
import { Check, Languages } from 'lucide-react'
import { LANGUAGE_OPTIONS, translateText, useI18n } from '@/app/use-i18n'
import { SettingsCard as Panel } from './settings-primitives'
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

  return (
    <Panel className="p-[18px]">
      <div className="flex items-start gap-[11px] [&_h2]:text-[16px] [&_p]:mt-1 [&_p]:text-[13px] [&_p]:leading-[1.55] [&_p]:text-[var(--text-muted)]">
        <span className="grid size-[38px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--star-soft)] text-[var(--star-strong)]">
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
        className="mt-[18px] grid grid-cols-2 gap-[9px] max-[650px]:grid-cols-1"
        role="radiogroup"
        aria-label={t('config:languageSettings.displayLanguage')}
      >
        {LANGUAGE_OPTIONS.map((option) => {
          const selected = option.value === language
          return (
            <button
              type="button"
              className={`grid min-h-[80px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 rounded-[var(--r-sm)] border bg-[var(--surface-subtle)] p-[11px] text-left text-[var(--text)] transition hover:-translate-y-px hover:border-[var(--star)] hover:bg-[var(--accent-soft)] ${selected ? 'border-[var(--star)] bg-[var(--star-soft)] shadow-[0_0_0_3px_var(--accent-ring)]' : 'border-[var(--stroke)]'}`}
              role="radio"
              aria-checked={selected}
              onClick={() => selectLanguage(option.value)}
              key={option.value}
            >
              <span
                className={`grid size-[34px] place-items-center rounded-[var(--r-xs)] text-[12px] font-extrabold ${option.value === 'en-US' ? 'bg-[var(--brand-blue-soft)] text-[var(--brand-blue-strong)]' : 'bg-[var(--solid)] text-[var(--star-strong)]'}`}
              >
                {option.shortName}
              </span>
              <span className="flex min-w-0 flex-col gap-[3px] [&_small]:text-[var(--text-muted)] [&_strong]:text-[13px]">
                <strong>{languageName(option.value, t)}</strong>
                <small>
                  {option.value === 'zh-CN'
                    ? t('config:languageSettings.chineseInterface')
                    : t('config:languageSettings.englishInterface')}
                </small>
              </span>
              {selected && (
                <span className="grid size-[21px] place-items-center rounded-full bg-[var(--star)] text-[var(--on-accent)]">
                  <Check size={15} />
                </span>
              )}
            </button>
          )
        })}
      </div>
      <AppNotice className="mt-[15px]">
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
    </Panel>
  )
}
