// 语言 Provider：挂载 i18next 实例，并把语言偏好同步到桌面壳：
// 写入 localStorage 持久化，同步 <html lang>，并通知 Tauri 桥接层
// 更新系统菜单/外壳文案的语言。
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
    // Keep desktop shell chrome and menus in sync with the UI language.
    void window.pisperDesktop?.setLanguage?.(language)
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
