import { Children, isValidElement, memo, useState, type ReactNode } from 'react'
import { Check, Copy } from 'lucide-react'
import ReactMarkdown, { type Components } from 'react-markdown'
import rehypeHighlight from 'rehype-highlight'
import remarkGfm from 'remark-gfm'
import type { Pluggable } from 'unified'
import { useI18n } from '@/app/use-i18n'
import { prepareMarkdown } from '@/lib/markdown'

const MARKDOWN_PLUGINS: Pluggable[] = [remarkGfm]
const HIGHLIGHT_PLUGINS: Pluggable[] = [[rehypeHighlight, { detect: false, ignoreMissing: true }]]
const MARKDOWN_COMPONENTS: Components = {
  a: ({ children: label, node: _node, className, href, ...props }) => (
    <a
      {...props}
      className={['markdown-link', className].filter(Boolean).join(' ')}
      href={href}
      target="_blank"
      rel="noreferrer"
    >
      {textContent(label).trim() ? label : href}
    </a>
  ),
  pre: ({ children: codeChildren }) => <CodeBlock>{codeChildren}</CodeBlock>,
  code: ({ children: code, className, node: _node, ...props }) => (
    <code className={className || ''} {...props}>
      {code}
    </code>
  ),
}

function textContent(value: ReactNode): string {
  if (typeof value === 'string' || typeof value === 'number') return String(value)
  if (Array.isArray(value)) return value.map(textContent).join('')
  if (isValidElement<{ children?: ReactNode }>(value)) return textContent(value.props.children)
  return ''
}

function languageName(className: string | undefined) {
  const match = String(className || '').match(/language-([\w-]+)/)
  return match?.[1] || 'text'
}

function copyText(value: string) {
  if (navigator.clipboard?.writeText) return navigator.clipboard.writeText(value)
  const textarea = document.createElement('textarea')
  textarea.value = value
  textarea.setAttribute('readonly', '')
  textarea.style.position = 'fixed'
  textarea.style.opacity = '0'
  document.body.append(textarea)
  textarea.select()
  document.execCommand('copy')
  textarea.remove()
  return Promise.resolve()
}

function CodeBlock({ children }: { children: ReactNode }) {
  const { t } = useI18n()
  const [copied, setCopied] = useState(false)
  const codeNode = Children.toArray(children).find((child) =>
    isValidElement<{ className?: string; children?: ReactNode }>(child),
  )
  const className = codeNode?.props.className || ''
  const source = textContent(codeNode?.props.children || children).replace(/\n$/, '')

  const copy = async () => {
    try {
      await copyText(source)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1800)
    } catch {
      setCopied(false)
    }
  }

  return (
    <div className="code-block">
      <div className="code-block-toolbar">
        <span>{languageName(className)}</span>
        <button
          type="button"
          onClick={copy}
          aria-label={t('common:markdownMessage.copyCode')}
          title={t('common:markdownMessage.copyCode')}
        >
          {copied ? <Check size={13} /> : <Copy size={13} />}
          {copied ? t('common:markdownMessage.copied') : t('common:markdownMessage.copy')}
        </button>
      </div>
      <pre>
        <code className={className}>{codeNode?.props.children || children}</code>
      </pre>
    </div>
  )
}

function MarkdownMessage({
  children,
  streaming = false,
}: {
  children: ReactNode
  streaming?: boolean
}) {
  const source = prepareMarkdown(children, streaming)
  // Streaming: skip full AST + syntax highlight. Rebuilding them on every SSE token causes flicker
  // (especially over remote desktops). Final message still gets full markdown rendering.
  if (streaming) {
    return (
      <div className="markdown-body markdown-streaming" aria-busy="true">
        <pre className="streaming-plain">{source}</pre>
      </div>
    )
  }
  return (
    <div className="markdown-body">
      <ReactMarkdown
        remarkPlugins={MARKDOWN_PLUGINS}
        rehypePlugins={HIGHLIGHT_PLUGINS}
        components={MARKDOWN_COMPONENTS}
      >
        {source}
      </ReactMarkdown>
    </div>
  )
}

export default memo(MarkdownMessage)
