import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

export const DEFAULT_LANGUAGE = 'zh-CN'
export const SUPPORTED_LANGUAGES = Object.freeze(['zh-CN', 'en-US'])

const catalogs = new Map()
let currentLanguage = DEFAULT_LANGUAGE

function localesDir() {
  return join(dirname(fileURLToPath(import.meta.url)), 'locales')
}

function loadCatalog(language) {
  if (catalogs.has(language)) return catalogs.get(language)
  try {
    const data = JSON.parse(readFileSync(join(localesDir(), `${language}.json`), 'utf8'))
    catalogs.set(language, data && typeof data === 'object' ? data : {})
  } catch {
    catalogs.set(language, {})
  }
  return catalogs.get(language)
}

export function isSupportedLanguage(language) {
  return typeof language === 'string' && SUPPORTED_LANGUAGES.includes(language)
}

export function getDesktopLanguage() {
  return currentLanguage
}

export function setDesktopLanguage(language) {
  if (!isSupportedLanguage(language)) return currentLanguage
  currentLanguage = language
  return currentLanguage
}

export function t(key, values = {}, language = currentLanguage) {
  const catalog = loadCatalog(language)
  const fallback = loadCatalog(DEFAULT_LANGUAGE)
  const template = catalog[key] ?? fallback[key] ?? key
  return String(template).replace(/\{(\w+)\}/g, (_, name) => {
    if (values[name] == null) return `{${name}}`
    return String(values[name])
  })
}
