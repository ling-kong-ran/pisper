// 语言偏好 hook：包装 react-i18next，返回当前语言与切换方法。
// 统一用 translateText 的固定语言路径，避免组件间因实例状态不一致
// 而读到不同语言；语言切换即时生效（changeLanguage）。
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

// 语言偏好 hook：从 i18next 实例解析当前语言（非法值回退默认），
// 提供 t/setLanguage；t 用固定语言路径，组件间不会因实例状态漂移。
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
