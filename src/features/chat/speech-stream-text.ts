import { parseFragment, type DefaultTreeAdapterTypes } from 'parse5'
import { htmlVoidElements } from 'html-void-elements'
import { abortReason, throwIfAborted } from '@/lib/abort-signal'
import {
  parseSpeechMarkdown,
  speechHiddenHtmlTags,
  speechTextFromMarkdownTree,
} from './speech-text'

type MarkdownNode = {
  type: string
  value?: string
  position?: { start: { offset?: number }; end: { offset?: number } }
  children?: MarkdownNode[]
}

const sentenceMarks = /[。！？.!?]/
const onlyPunctuation = /^[\p{P}\p{Z}\s]+$/u
const words = new Intl.Segmenter('zh-CN', { granularity: 'word' })
const abbreviations = new Set([
  'mr',
  'mrs',
  'ms',
  'dr',
  'prof',
  'sr',
  'jr',
  'st',
  'vs',
  'etc',
  'fig',
  'no',
  'dept',
  'inc',
  'ltd',
  'approx',
  'e.g',
  'i.e',
])

function isAbbreviation(prefix: string): boolean {
  const token = /([a-z]+(?:\.[a-z]+)*)\.$/i.exec(prefix)?.[1] ?? ''
  return (
    abbreviations.has(token.toLowerCase()) ||
    /^(?:[a-z]\.)+[a-z]$/i.test(token) ||
    /^[A-Z]$/.test(token)
  )
}

function uncertainTextOffset(source: string): number {
  for (let index = 0; index < source.length; index++) {
    const character = source[index]
    if (character === '\\') {
      if (index + 1 === source.length) return index
      index++
    } else if (character === '!' && source[index + 1] === '[') return index
    else if ('[`<*_~|'.includes(character)) return index
    else if (character === '&') {
      const entity = /^&(?:#[0-9]+|#x[\da-f]+|[a-z][\da-z]+);/i.exec(source.slice(index))
      if (!entity) return index
      index += entity[0].length - 1
    }
  }
  return source.length
}

function htmlBoundary(tree: MarkdownNode, length: number) {
  const spans: { from: number; to: number; start: number; end: number }[] = []
  let html = ''
  const gather = (node: MarkdownNode) => {
    if (node.type === 'html') {
      const from = html.length
      html += node.value || ''
      spans.push({
        from,
        to: html.length,
        start: node.position?.start.offset ?? 0,
        end: node.position?.end.offset ?? length,
      })
      html += '\n'
    }
    node.children?.forEach(gather)
  }
  gather(tree)
  let horizon = length
  const hidden: { start: number; end: number }[] = []
  if (!spans.length) return { horizon, hidden }
  const spanAt = (offset: number) =>
    spans.find((span) => offset >= span.from && offset < span.to) ?? spans.at(-1)!
  // 仅投影解析器认可的 HTML；代码、转义及链接内的伪闭合标签不能改变 HTML 状态。
  const fragment = parseFragment(html, {
    sourceCodeLocationInfo: true,
    scriptingEnabled: false,
    onParseError(error) {
      if (error.code.startsWith('eof-'))
        horizon = Math.min(horizon, spanAt(error.startOffset).start)
    },
  })
  const inspect = (node: DefaultTreeAdapterTypes.Node) => {
    if ('tagName' in node) {
      const location = node.sourceCodeLocation
      if (location?.startTag) {
        const start = spanAt(location.startOffset).start
        const empty =
          node.namespaceURI === 'http://www.w3.org/1999/xhtml'
            ? htmlVoidElements.includes(node.tagName)
            : html.slice(location.startTag.startOffset, location.startTag.endOffset).endsWith('/>')
        if (!location.endTag && !empty) horizon = Math.min(horizon, start)
        if (speechHiddenHtmlTags.has(node.tagName))
          hidden.push({
            start,
            end: location.endTag
              ? spanAt(location.endTag.endOffset - 1).end
              : empty
                ? spanAt(location.startTag.endOffset - 1).end
                : length,
          })
      }
    }
    if ('childNodes' in node) node.childNodes.forEach(inspect)
    if ('content' in node) inspect(node.content)
  }
  inspect(fragment)
  return { horizon, hidden }
}

function stableText(markdown: string, complete: boolean): string {
  const tree = parseSpeechMarkdown(markdown)
  if (!complete) {
    const boundary = htmlBoundary(tree, markdown.length)
    let horizon = boundary.horizon
    const inspect = (node: MarkdownNode) => {
      const start = node.position?.start.offset ?? 0
      const end = node.position?.end.offset ?? start
      // 隐藏元素内部的 Markdown 不会发音，也不能把已闭合 HTML 后的正文永久挡住。
      if (boundary.hidden.some((range) => start >= range.start && end <= range.end)) return
      // 引用定义可在后文出现；仅对解析器仍视为文本的潜在标记设置保守屏障。
      if (node.type === 'text') {
        const source = markdown.slice(start, end)
        const uncertain = uncertainTextOffset(source)
        if (uncertain < source.length) horizon = Math.min(horizon, start + uncertain)
      }
      node.children?.forEach(inspect)
    }
    inspect(tree)
    const truncate = (node: MarkdownNode) => {
      if (node.children) {
        node.children = node.children.filter(
          (child) => (child.position?.start.offset ?? 0) < horizon,
        )
        node.children.forEach(truncate)
      }
      if (node.type === 'text' && (node.position?.end.offset ?? 0) > horizon) {
        const prefix = markdown.slice(node.position?.start.offset ?? 0, horizon)
        // 加普通文本前缀，阻止内联片段被单独解析成标题或列表；实体与转义仍由解析器解码。
        node.value = speechTextFromMarkdownTree(parseSpeechMarkdown(`x${prefix}`)).slice(1)
      }
    }
    truncate(tree)
  }
  return speechTextFromMarkdownTree(tree).replace(/\s+/g, ' ').trim()
}

function sentenceEnd(text: string, start: number, complete: boolean): number | undefined {
  for (let index = start; index < text.length; index++) {
    if (!sentenceMarks.test(text[index])) continue
    if (text[index] === '.') {
      if (index + 1 === text.length && !complete) return
      if (/[\p{L}\p{N}]/u.test(text[index + 1] || '')) continue
      if (isAbbreviation(text.slice(start, index + 1))) continue
    }
    let end = index + 1
    while (end < text.length && /[。！？.!?"'”’）)\]}»]/.test(text[end])) end++
    if (!onlyPunctuation.test(text.slice(start, end))) return end
    index = end - 1
  }
  if (complete && text.slice(start).trim()) return text.length
}

function boundedSentence(sentence: string, limit: number): string[] {
  if (onlyPunctuation.test(sentence)) return []
  const units: string[] = []
  for (const { segment } of words.segment(sentence.trim())) {
    if (onlyPunctuation.test(segment) && segment.trim() && units.at(-1)?.trim())
      units[units.length - 1] += segment
    else if (segment.trim() && units.at(-1)?.trim() && onlyPunctuation.test(units.at(-1)!))
      units[units.length - 1] += segment
    else units.push(segment)
  }
  const result: string[] = []
  let current = ''
  const flush = () => {
    if (current.trim()) result.push(current.trim())
    current = ''
  }
  for (const unit of units) {
    if (!current && !unit.trim()) continue
    const characters = Array.from(unit)
    if (characters.length > limit) {
      flush()
      const suffix = /[\p{P}]+$/u.exec(unit)?.[0] ?? ''
      const suffixLength = Array.from(suffix).length
      if (suffixLength >= limit)
        throw new Error('Speech punctuation exceeds the supported segment limit.')
      while (characters.length > limit) {
        const count = Math.min(limit, characters.length - suffixLength - 1)
        result.push(characters.splice(0, count).join(''))
      }
      current = characters.join('')
    } else {
      if (Array.from(current + unit).length > limit) flush()
      if (current || unit.trim()) current += unit
    }
  }
  flush()
  // 引号或分隔符不能单独触发合成；能容纳时并回相邻正文，否则明确报预算错误。
  for (let index = 0; index < result.length; index++) {
    if (!onlyPunctuation.test(result[index])) continue
    const neighbor = index > 0 ? index - 1 : index + 1
    if (result[neighbor] === undefined) return []
    const joined = index > 0 ? result[neighbor] + result[index] : result[index] + result[neighbor]
    if (Array.from(joined).length > limit)
      throw new Error('Speech punctuation exceeds the supported segment limit.')
    result[neighbor] = joined
    result.splice(index, 1)
    index--
  }
  return result
}

function reconcileEmitted(text: string, emitted: string): string {
  if (text.startsWith(emitted)) return emitted
  let cursor = 0
  // 即时 ! 后到达的 [ 可将标点改判为图片标记；只允许标点、空白变化，正文仍须严格匹配。
  for (const character of emitted) {
    if (onlyPunctuation.test(character)) continue
    while (cursor < text.length && onlyPunctuation.test(text[cursor])) cursor++
    if (!text.startsWith(character, cursor)) throw new Error('Speech text changed after emission.')
    cursor += character.length
  }
  while (cursor < text.length && onlyPunctuation.test(text[cursor])) cursor++
  return text.slice(0, cursor)
}

function nextChunk(iterator: AsyncIterator<string>, signal: AbortSignal) {
  throwIfAborted(signal)
  return new Promise<IteratorResult<string>>((resolve, reject) => {
    const abort = () => {
      signal.removeEventListener('abort', abort)
      reject(abortReason(signal))
    }
    signal.addEventListener('abort', abort, { once: true })
    // 同时观察迟到成功和失败，取消后源 next() 不配合也不会挂住消费者或产生未处理拒绝。
    Promise.resolve()
      .then(() => {
        throwIfAborted(signal)
        return iterator.next()
      })
      .then(resolve, reject)
      .finally(() => signal.removeEventListener('abort', abort))
  })
}

export async function* streamingSpeechSegments(
  chunks: AsyncIterable<string>,
  signal: AbortSignal,
  limit = 16,
): AsyncGenerator<string> {
  throwIfAborted(signal)
  if (!Number.isSafeInteger(limit) || limit < 8 || limit > 160)
    throw new RangeError('Invalid speech segment limit.')
  const iterator = chunks[Symbol.asyncIterator]()
  let markdown = ''
  let pendingSurrogate = ''
  let text = ''
  let emitted = ''
  let count = 0
  let received = 0
  let complete = false
  let pendingBoundary = false
  try {
    for (;;) {
      throwIfAborted(signal)
      const end = sentenceEnd(text, emitted.length, complete)
      if (end !== undefined) {
        const segments = boundedSentence(text.slice(emitted.length, end), limit)
        if (count + segments.length > 512)
          throw new Error('Speech response contains too many segments.')
        count += segments.length
        emitted = text.slice(0, end)
        pendingBoundary = sentenceMarks.test(text.slice(end))
        // 每次仅展开当前句子的段；消费者暂停时不继续拉取源或处理其余句子。
        for (const segment of segments) {
          throwIfAborted(signal)
          yield segment
        }
        continue
      }
      if (complete) return
      const chunk = await nextChunk(iterator, signal)
      throwIfAborted(signal)
      complete = Boolean(chunk.done)
      if (!complete) {
        if (typeof chunk.value !== 'string') throw new TypeError('Invalid speech text delta.')
        received += chunk.value.length
        if (received > 32_000) throw new Error('Speech text exceeds the supported limit.')
        let value = pendingSurrogate + chunk.value
        pendingSurrogate = ''
        if (/[\uD800-\uDBFF]$/.test(value)) {
          pendingSurrogate = value.slice(-1)
          value = value.slice(0, -1)
        }
        if (/[\uD800-\uDFFF]/u.test(value)) throw new Error('Invalid speech text Unicode.')
        markdown += value
        pendingBoundary ||=
          sentenceMarks.test(value) ||
          (/[&;`*_~>|\])\n]/.test(value) && /[。！？.!?&]/.test(markdown))
      } else if (pendingSurrogate) throw new Error('Invalid speech text Unicode.')
      // 没有句末时无需反复解析逐字 delta；自然句末立即建立快照，仅 ASCII 句点需要后继。
      if (!complete && !pendingBoundary) continue
      text = stableText(markdown, complete)
      emitted = reconcileEmitted(text, emitted)
      pendingBoundary = sentenceMarks.test(text.slice(emitted.length))
    }
  } finally {
    if (!complete) {
      try {
        // 异步生成器的 return 可能排在挂起 next 后；调用清理但不能等待不合作的源。
        void Promise.resolve(iterator.return?.()).catch(() => {})
      } catch {
        // 保留原始取消或读取错误，不用源清理错误覆盖它。
      }
    }
  }
}
