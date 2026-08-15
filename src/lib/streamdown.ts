import { cjk } from '@streamdown/cjk'
import type { HighlightResult } from '@streamdown/code'
import { math } from '@streamdown/math'
import {
  bundledLanguagesInfo,
  createHighlighter,
  type BundledLanguage,
  type Highlighter,
} from 'shiki'
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript'
import {
  parseMarkdownIntoBlocks,
  type CodeHighlighterPlugin,
  type PluginConfig,
  type ThemeInput,
} from 'streamdown'

/**
 * @streamdown/code 的官方插件用两个永不淘汰的 Map 缓存 highlighter 实例和
 * token 结果（key 包含代码内容）。流式输出时代码块随 typewriter 不断增长，
 * 每个中间版本都会产生一条永久驻留的缓存，是长会话渲染进程内存膨胀的
 * 主要来源。这里改为单例 highlighter + 有界 LRU 缓存，把驻留内存限制在
 * 固定上限内；语言 grammar 仍按需动态加载。
 */

const SHIKI_THEMES: [ThemeInput, ThemeInput] = ['github-dark', 'github-dark']
const MAX_TOKEN_CACHE_ENTRIES = 150

const PLAINTEXT_LANGUAGES = new Set(['text', 'txt', 'plain', 'plaintext'])

const languageAlias = new Map<string, string>()
for (const info of bundledLanguagesInfo) {
  languageAlias.set(info.id, info.id)
  for (const alias of info.aliases ?? []) languageAlias.set(alias, info.id)
}

function themeName(theme: ThemeInput) {
  return typeof theme === 'string' ? theme : (theme.name ?? 'custom')
}

function resolveLanguage(language: unknown) {
  const normalized = String(language || '')
    .trim()
    .toLowerCase()
  return languageAlias.get(normalized) ?? 'text'
}

function tokenCacheKey(code: string, language: string, themes: [ThemeInput, ThemeInput]) {
  // 与官方插件相同的思路：长度 + 首尾片段区分内容，避免把整段代码复制进 key。
  return `${language}:${themeName(themes[0])}:${themeName(themes[1])}:${code.length}:${code.slice(0, 64)}:${code.slice(-64)}`
}

const tokenCache = new Map<string, HighlightResult>()

function readTokenCache(key: string) {
  const cached = tokenCache.get(key)
  if (cached) {
    // LRU：命中后移到最新位置。
    tokenCache.delete(key)
    tokenCache.set(key, cached)
  }
  return cached
}

function writeTokenCache(key: string, result: HighlightResult) {
  tokenCache.delete(key)
  tokenCache.set(key, result)
  while (tokenCache.size > MAX_TOKEN_CACHE_ENTRIES) {
    const oldest = tokenCache.keys().next().value
    if (oldest === undefined) break
    tokenCache.delete(oldest)
  }
}

let highlighterPromise: Promise<Highlighter> | null = null
const languageLoads = new Map<string, Promise<unknown>>()

function getHighlighter() {
  highlighterPromise ??= createHighlighter({
    themes: [...SHIKI_THEMES],
    langs: [],
    engine: createJavaScriptRegexEngine({ forgiving: true }),
  })
  return highlighterPromise
}

function ensureLanguageLoaded(highlighter: Highlighter, language: string): Promise<unknown> {
  if (PLAINTEXT_LANGUAGES.has(language) || highlighter.getLoadedLanguages().includes(language))
    return Promise.resolve()
  const info = bundledLanguagesInfo.find((item) => item.id === language)
  if (!info) return Promise.resolve()
  let load = languageLoads.get(language)
  if (!load) {
    load = highlighter
      .loadLanguage(() => info.import().then((module) => module.default))
      .catch((error: unknown) => {
        languageLoads.delete(language)
        throw error
      })
    languageLoads.set(language, load)
  }
  return load
}

const pendingHighlights = new Map<string, Set<(result: HighlightResult) => void>>()

function createBoundedCodePlugin(): CodeHighlighterPlugin {
  return {
    name: 'shiki',
    type: 'code-highlighter',
    supportsLanguage(language) {
      return languageAlias.has(
        String(language || '')
          .trim()
          .toLowerCase(),
      )
    },
    getSupportedLanguages() {
      return bundledLanguagesInfo.map((info) => info.id as BundledLanguage)
    },
    getThemes() {
      return SHIKI_THEMES
    },
    highlight({ code, language, themes }, callback) {
      const resolved = resolveLanguage(language)
      const key = tokenCacheKey(code, resolved, themes)
      const cached = readTokenCache(key)
      if (cached) return cached

      let callbacks = pendingHighlights.get(key)
      const alreadyRunning = Boolean(callbacks)
      if (!callbacks) {
        callbacks = new Set()
        pendingHighlights.set(key, callbacks)
      }
      if (callback) callbacks.add(callback)
      if (alreadyRunning) return null

      void (async () => {
        try {
          const highlighter = await getHighlighter()
          await ensureLanguageLoaded(highlighter, resolved)
          const loaded = highlighter.getLoadedLanguages().includes(resolved) ? resolved : 'text'
          const result = highlighter.codeToTokens(code, {
            lang: loaded as BundledLanguage,
            themes: { light: themes[0], dark: themes[1] },
          })
          writeTokenCache(key, result)
          const waiting = pendingHighlights.get(key)
          pendingHighlights.delete(key)
          if (waiting) for (const notify of waiting) notify(result)
        } catch (error) {
          pendingHighlights.delete(key)
          console.error('[streamdown] Failed to highlight code:', error)
        }
      })()
      return null
    },
  }
}

const streamdownCode = createBoundedCodePlugin()

export const streamdownPlugins = { cjk, code: streamdownCode, math } satisfies PluginConfig

/**
 * Streaming markdown 是逐字追加的：前面的 block 已经稳定，只有最后一个 block
 * 在增长。这个解析器利用该特性，只重解析最后一个 block 的尾部，避免每次增量都
 * 用 marked Lexer 重解析整段文本（高频分配字符串/数组/token 的根源）。
 */
export function createIncrementalBlockParser() {
  let prevSource = ''
  let prevBlocks: string[] = []

  return (markdown: string): string[] => {
    if (markdown === prevSource) return prevBlocks

    // 纯追加：新文本以旧文本为前缀（typewriter 的常见路径）。
    if (prevBlocks.length > 0 && markdown.startsWith(prevSource)) {
      const lastBlock = prevBlocks[prevBlocks.length - 1]
      const tailStart = prevSource.length - lastBlock.length
      const tailBlocks = parseMarkdownIntoBlocks(markdown.slice(tailStart))
      const merged = [...prevBlocks.slice(0, -1), ...tailBlocks]
      prevSource = markdown
      prevBlocks = merged
      return merged
    }

    // 回退到全量解析（首帧、文本重写/回退、跨消息复用等）。
    prevSource = markdown
    prevBlocks = parseMarkdownIntoBlocks(markdown)
    return prevBlocks
  }
}
