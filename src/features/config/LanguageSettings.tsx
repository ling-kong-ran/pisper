// 语言设置独立成卡片，避免与外观偏好耦合；切换后即时同步应用语言。
import { useId } from 'react'
import { LANGUAGE_OPTIONS, translateText, useI18n } from '@/app/use-i18n'
import type { SupportedLanguage } from '@/app/i18n'
import type { Notify } from '@/app/route-context'

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'

function languageName(language: SupportedLanguage, t: ReturnType<typeof useI18n>['t']) {
  return language === 'en-US'
    ? t('config:languageSettings.english')
    : t('config:languageSettings.simplifiedChinese')
}

export function LanguageSettings({ notify }: { notify: Notify }) {
  const { language, setLanguage, t } = useI18n()
  const labelId = useId()
  const descriptionId = useId()

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
    // 与界面设置的其他行同一层级，不再单独套卡片。
    <div
      className="flex flex-row flex-wrap items-center justify-between gap-x-6 gap-y-3 border-t border-border py-4"
      data-config-card="interface-language"
    >
      <div className="min-w-0 flex-1 basis-40">
        <h2 id={labelId} className="text-sm font-medium">
          {t('config:languageSettings.displayLanguage')}
        </h2>
        <p id={descriptionId} className="mt-1 text-xs leading-relaxed text-muted-foreground">
          {t('config:languageSettings.interfaceOnlyHint')}
        </p>
      </div>
      <Select
        value={language}
        onValueChange={(next) => {
          const option = LANGUAGE_OPTIONS.find((item) => item.value === next)
          if (option) selectLanguage(option.value)
        }}
      >
        <SelectTrigger
          aria-labelledby={labelId}
          aria-describedby={descriptionId}
          className="min-h-9 min-w-32 shrink-0 rounded-full px-3 max-[650px]:min-h-11"
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent position="popper" align="end" className="p-1">
          {LANGUAGE_OPTIONS.map((option) => (
            <SelectItem
              key={option.value}
              value={option.value}
              className="min-h-9 px-3 pr-8 max-[650px]:min-h-11"
            >
              {languageName(option.value, t)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  )
}
