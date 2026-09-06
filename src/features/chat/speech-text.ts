import { fromMarkdown } from 'mdast-util-from-markdown'
import { gfmFromMarkdown } from 'mdast-util-gfm'
import { gfm } from 'micromark-extension-gfm'
import { toHast } from 'mdast-util-to-hast'
import { raw } from 'hast-util-raw'

type TextNode = { type: string; tagName?: string; value?: string; children?: TextNode[] }

function excludeBlocks(node: TextNode) {
  const inline = [
    'paragraph',
    'heading',
    'emphasis',
    'strong',
    'delete',
    'link',
    'linkReference',
  ].includes(node.type)
  if (node.children) {
    node.children = node.children.filter(
      (child) =>
        !['code', 'image', 'imageReference', 'footnoteDefinition', 'footnoteReference'].includes(
          child.type,
        ) &&
        (child.type !== 'html' || inline),
    )
    node.children.forEach(excludeBlocks)
  }
}

export const speechHiddenHtmlTags = new Set([
  'script',
  'style',
  'template',
  'iframe',
  'object',
  'svg',
  'math',
  'noscript',
  'pre',
  'img',
])

function textOf(node: TextNode): string {
  if (speechHiddenHtmlTags.has(node.tagName || '')) return ''
  if (node.type === 'text') return node.value || ''
  const text = node.children?.map(textOf).join('') || ''
  if (node.tagName === 'code') return text.length <= 60 && !/[{};=<>\n]/.test(text) ? text : ''
  return [
    'p',
    'h1',
    'h2',
    'h3',
    'h4',
    'h5',
    'h6',
    'li',
    'tr',
    'td',
    'th',
    'blockquote',
    'br',
  ].includes(node.tagName || '')
    ? `${text}\n`
    : text
}

function speechCost(value: string) {
  return Array.from(value).length
}

export function parseSpeechMarkdown(markdown: string) {
  return fromMarkdown(markdown, { extensions: [gfm()], mdastExtensions: [gfmFromMarkdown()] })
}

export function speechTextFromMarkdownTree(tree: ReturnType<typeof parseSpeechMarkdown>) {
  excludeBlocks(tree)
  // 流式和完整回复共用清洗规则，避免分段后再次解析改变纯文本含义。
  return textOf(raw(toHast(tree, { allowDangerousHtml: true })))
    .replace(/https?:\/\/[^\s]+/g, '')
    .replace(/\p{Extended_Pictographic}/gu, '')
}

// Markdown 用已有解析器处理；分句按词边界保留全文，避免把代码围栏或半个代理对送入 TTS。
export function speechSegments(markdown: string, limit = 16) {
  if (markdown.length > 32_000 || /[\uD800-\uDFFF]/u.test(markdown))
    throw new Error('Speech text exceeds the supported limit.')
  if (!Number.isSafeInteger(limit) || limit < 8 || limit > 160)
    throw new RangeError('Invalid speech segment limit.')
  const text = speechTextFromMarkdownTree(parseSpeechMarkdown(markdown))
  const words = new Intl.Segmenter('zh-CN', { granularity: 'word' })
  const result: string[] = []
  let current = ''
  const flush = () => {
    if (current.trim()) result.push(current.trim())
    current = ''
  }
  for (const line of text.split(/\n+/)) {
    for (const { segment } of words.segment(line.replace(/\s+/g, ' ').trim())) {
      if (!current && !segment.trim()) continue
      if (speechCost(segment) > limit) {
        flush()
        for (const character of segment) {
          if (speechCost(current + character) > limit) flush()
          current += character
        }
      } else {
        if (speechCost(current + segment) > limit) {
          if (/^\p{P}+$/u.test(segment) && current.trim()) {
            // 标点必须随有声文字成段；优先把最后一个完整词移到下一段。
            const previous = [...words.segment(current.trimEnd())].at(-1)!
            const prefix = current.slice(0, previous.index).trimEnd()
            if (prefix && speechCost(previous.segment + segment) <= limit) {
              current = prefix
              flush()
              current = previous.segment
            } else {
              const characters = Array.from(current.trimEnd())
              const tail = characters.splice(-speechCost(segment)).join('')
              current = characters.join('')
              flush()
              current = tail
            }
          } else flush()
        }
        current += segment
      }
      if (/[。！？!?；;，,]/.test(segment) || segment === '.') flush()
    }
    flush()
  }
  if (result.length > 512) throw new Error('Speech response contains too many segments.')
  return result
}
