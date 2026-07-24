import { Check, FoldVertical, Languages, UnfoldVertical } from 'lucide-react'
import { LANGUAGE_OPTIONS, translateText, useI18n } from '../../app/use-i18n'
import { Panel } from '../../components/ui'
import { useUiStore, type DensityMode } from '../../stores/ui-store'
import type { LucideIcon } from 'lucide-react'
import type { SupportedLanguage } from '../../app/i18n'
import type { Notify } from '../../app/route-context'

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
    <div className="language-settings">
      <Panel className="language-settings-card">
        <div className="language-settings-heading">
          <span className="language-settings-icon">
            <Languages size={19} />
          </span>
          <div>
            <h2>{t('config:languageSettings.displayLanguage')}</h2>
            <p>
              {t(
                'config:languageSettings.chooseTheLanguageUsedByVesperChangesApplyImmediatelyAndAreSavedForYourNextVisit',
              )}
            </p>
          </div>
        </div>
        <div
          className="language-choice-grid"
          role="radiogroup"
          aria-label={t('config:languageSettings.displayLanguage')}
        >
          {LANGUAGE_OPTIONS.map((option) => {
            const selected = option.value === language
            return (
              <button
                type="button"
                className={`language-choice ${selected ? 'selected' : ''}`}
                role="radio"
                aria-checked={selected}
                onClick={() => selectLanguage(option.value)}
                key={option.value}
              >
                <span
                  className={`language-choice-mark ${option.value === 'en-US' ? 'english' : ''}`}
                >
                  {option.shortName}
                </span>
                <span className="language-choice-copy">
                  <strong>{languageName(option.value, t)}</strong>
                  <small>
                    {option.value === 'zh-CN'
                      ? t('config:languageSettings.chineseInterface')
                      : t('config:languageSettings.englishInterface')}
                  </small>
                </span>
                {selected && (
                  <span className="language-choice-check">
                    <Check size={15} />
                  </span>
                )}
              </button>
            )
          })}
        </div>
        <div className="permission-note language-settings-note">
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
        </div>
        <small className="language-settings-storage">
          {t('config:languageSettings.languagePreferenceIsStoredInThisBrowser')}
        </small>
      </Panel>
      <Panel className="language-settings-card density-settings-card">
        <div className="language-settings-heading">
          <span className="language-settings-icon">
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
          className="language-choice-grid"
          role="radiogroup"
          aria-label={t('config:languageSettings.interfaceDensity')}
        >
          {densityOptions.map(([value, label, description, Icon]) => {
            const selected = value === density
            return (
              <button
                type="button"
                className={`language-choice ${selected ? 'selected' : ''}`}
                role="radio"
                aria-checked={selected}
                onClick={() => selectDensity(value)}
                key={value}
              >
                <span className="language-choice-mark">
                  <Icon size={16} />
                </span>
                <span className="language-choice-copy">
                  <strong>{label}</strong>
                  <small>{description}</small>
                </span>
                {selected && (
                  <span className="language-choice-check">
                    <Check size={15} />
                  </span>
                )}
              </button>
            )
          })}
        </div>
        <div className="permission-note language-settings-note">
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
        </div>
      </Panel>
    </div>
  )
}
