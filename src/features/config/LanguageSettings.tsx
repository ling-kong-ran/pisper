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
    // 单行细线卡：与外观分组同一视觉层级，标题 13/500、说明 12px，控件右对齐。
    <div
      className="flex flex-row flex-wrap items-center justify-between gap-x-6 gap-y-3 rounded-md border border-stroke bg-panel px-4 py-3"
      data-config-card="interface-language"
    >
      <div className="min-w-0 flex-1 basis-40">
        <h2 id={labelId} className="text-sm font-medium text-content">
          {t('config:languageSettings.displayLanguage')}
        </h2>
        <p id={descriptionId} className="mt-0.5 text-xs leading-relaxed text-content-muted">
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
          className="min-w-36 shrink-0 pointer-coarse:min-h-11 max-[650px]:min-h-11"
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
