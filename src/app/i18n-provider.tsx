import { useEffect, type ReactNode } from 'react'
import { I18nextProvider, useTranslation } from 'react-i18next'
import { STORAGE_KEYS } from './storage'
import { DEFAULT_LANGUAGE, isSupportedLanguage, i18n } from './i18n'

function LanguagePreferenceBridge({ children }: { children: ReactNode }) {
  const { i18n: instance } = useTranslation()
  const language = isSupportedLanguage(instance.resolvedLanguage)
    ? instance.resolvedLanguage
    : DEFAULT_LANGUAGE

  useEffect(() => {
    document.documentElement.lang = language
    try {
      localStorage.setItem(STORAGE_KEYS.language, language)
    } catch {}
    // Keep Electron shell chrome (tray, close dialog) in sync with the UI language.
    void window.vesperDesktop?.setLanguage?.(language)
  }, [language])

  return children
}

export function LanguageProvider({ children }: { children: ReactNode }) {
  return (
    <I18nextProvider i18n={i18n}>
      <LanguagePreferenceBridge>{children}</LanguagePreferenceBridge>
    </I18nextProvider>
  )
}
