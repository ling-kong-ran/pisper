import i18next from 'i18next'
import { initReactI18next } from 'react-i18next'
import enAssets from '../locales/en-US/assets.json' with { type: 'json' }
import enChannels from '../locales/en-US/channels.json' with { type: 'json' }
import enChat from '../locales/en-US/chat.json' with { type: 'json' }
import enCommon from '../locales/en-US/common.json' with { type: 'json' }
import enConfig from '../locales/en-US/config.json' with { type: 'json' }
import enMemory from '../locales/en-US/memory.json' with { type: 'json' }
import enNavigation from '../locales/en-US/navigation.json' with { type: 'json' }
import enPlugins from '../locales/en-US/plugins.json' with { type: 'json' }
import enSchedules from '../locales/en-US/schedules.json' with { type: 'json' }
import enSkills from '../locales/en-US/skills.json' with { type: 'json' }
import enWorkflows from '../locales/en-US/workflows.json' with { type: 'json' }
import zhAssets from '../locales/zh-CN/assets.json' with { type: 'json' }
import zhChannels from '../locales/zh-CN/channels.json' with { type: 'json' }
import zhChat from '../locales/zh-CN/chat.json' with { type: 'json' }
import zhCommon from '../locales/zh-CN/common.json' with { type: 'json' }
import zhConfig from '../locales/zh-CN/config.json' with { type: 'json' }
import zhMemory from '../locales/zh-CN/memory.json' with { type: 'json' }
import zhNavigation from '../locales/zh-CN/navigation.json' with { type: 'json' }
import zhPlugins from '../locales/zh-CN/plugins.json' with { type: 'json' }
import zhSchedules from '../locales/zh-CN/schedules.json' with { type: 'json' }
import zhSkills from '../locales/zh-CN/skills.json' with { type: 'json' }
import zhWorkflows from '../locales/zh-CN/workflows.json' with { type: 'json' }
import { STORAGE_KEYS } from './storage.ts'

export const DEFAULT_LANGUAGE = 'zh-CN' as const
export const SUPPORTED_LANGUAGES = ['zh-CN', 'en-US'] as const
export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number]
export type I18nValues = Record<string, unknown>

export function isSupportedLanguage(language: unknown): language is SupportedLanguage {
  return typeof language === 'string' && SUPPORTED_LANGUAGES.includes(language as SupportedLanguage)
}

export const LANGUAGE_OPTIONS = [
  { value: 'zh-CN', shortName: '中文' },
  { value: 'en-US', shortName: 'EN' },
] as const

export const I18N_NAMESPACES = Object.freeze([
  'assets',
  'channels',
  'common',
  'navigation',
  'chat',
  'config',
  'memory',
  'plugins',
  'schedules',
  'skills',
  'workflows',
] as const)
export type I18nNamespace = (typeof I18N_NAMESPACES)[number]

export function storedLanguage(): SupportedLanguage {
  try {
    const stored = localStorage.getItem(STORAGE_KEYS.language)
    return isSupportedLanguage(stored) ? stored : DEFAULT_LANGUAGE
  } catch {
    return DEFAULT_LANGUAGE
  }
}

export const i18n = i18next.createInstance()

void i18n.use(initReactI18next).init({
  lng: storedLanguage(),
  fallbackLng: DEFAULT_LANGUAGE,
  supportedLngs: SUPPORTED_LANGUAGES,
  defaultNS: 'common',
  ns: I18N_NAMESPACES,
  resources: {
    'zh-CN': {
      assets: zhAssets,
      channels: zhChannels,
      common: zhCommon,
      navigation: zhNavigation,
      chat: zhChat,
      config: zhConfig,
      memory: zhMemory,
      plugins: zhPlugins,
      schedules: zhSchedules,
      skills: zhSkills,
      workflows: zhWorkflows,
    },
    'en-US': {
      assets: enAssets,
      channels: enChannels,
      common: enCommon,
      navigation: enNavigation,
      chat: enChat,
      config: enConfig,
      memory: enMemory,
      plugins: enPlugins,
      schedules: enSchedules,
      skills: enSkills,
      workflows: enWorkflows,
    },
  },
  interpolation: {
    prefix: '{',
    suffix: '}',
    escapeValue: false,
  },
  keySeparator: false,
  nsSeparator: ':',
  initAsync: false,
})

export function translateText(
  message: string,
  language: SupportedLanguage = DEFAULT_LANGUAGE,
  values?: I18nValues,
): string {
  return i18n.getFixedT(language)(message, values)
}
