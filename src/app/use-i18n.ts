import { useCallback } from 'react'
import { useTranslation } from 'react-i18next'
import {
  DEFAULT_LANGUAGE,
  isSupportedLanguage,
  translateText,
  type I18nValues,
  type SupportedLanguage,
} from './i18n'

export {
  DEFAULT_LANGUAGE,
  I18N_NAMESPACES,
  LANGUAGE_OPTIONS,
  SUPPORTED_LANGUAGES,
  translateText,
} from './i18n'

export function useI18n() {
  const { i18n: instance } = useTranslation()
  const language = isSupportedLanguage(instance.resolvedLanguage)
    ? instance.resolvedLanguage
    : DEFAULT_LANGUAGE
  const setLanguage = useCallback(
    (nextLanguage: SupportedLanguage) => {
      if (!isSupportedLanguage(nextLanguage)) return
      void instance.changeLanguage(nextLanguage)
    },
    [instance],
  )
  const t = useCallback(
    (message: string, values?: I18nValues) => translateText(message, language, values),
    [language],
  )
  return { language, locale: language, setLanguage, t }
}
