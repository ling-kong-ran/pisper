import { isValidElement, memo, useState, type ComponentProps, type ReactNode } from 'react'
import { Check, Copy } from 'lucide-react'
import { CodeBlock, Streamdown, useIsCodeFenceIncomplete, type Components } from 'streamdown'
import { useI18n } from '@/app/use-i18n'
import { streamdownPlugins } from '@/lib/streamdown'
import { cn } from '@/lib/utils'

const MARKDOWN_COMPONENTS: Components = {
  a: MarkdownLink,
  code: MarkdownCode,
  img: MarkdownImage,
  strong: MarkdownStrong,
  table: MarkdownTable,
}
const STREAMDOWN_CONTROLS = false
const STREAMDOWN_REMEND = { linkMode: 'text-only' } as const

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

function MarkdownLink({
  children: label,
  node: _node,
  className,
  href,
  ...props
}: ComponentProps<'a'> & { node?: unknown }) {
  const content = textContent(label).trim() ? label : href
  if (!href) return <span className={cn('markdown-link', className)}>{content}</span>

  return (
    <a
      {...props}
      className={cn('markdown-link', className)}
      href={href}
      target="_blank"
      rel="noopener noreferrer"
    >
      {content}
    </a>
  )
}

function MarkdownImage({ node: _node, ...props }: ComponentProps<'img'> & { node?: unknown }) {
  return <img loading="lazy" {...props} />
}

function MarkdownStrong({ node: _node, ...props }: ComponentProps<'strong'> & { node?: unknown }) {
  return <strong {...props} />
}

function MarkdownTable({ node: _node, ...props }: ComponentProps<'table'> & { node?: unknown }) {
  return <table {...props} />
}

function MarkdownCopyButton({ source }: { source: string }) {
  const { t } = useI18n()
  const [copied, setCopied] = useState(false)

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
    <button
      type="button"
      data-streamdown="code-block-copy-button"
      onClick={copy}
      aria-label={t('common:markdownMessage.copyCode')}
      title={t('common:markdownMessage.copyCode')}
    >
      {copied ? <Check size={13} /> : <Copy size={13} />}
      {copied ? t('common:markdownMessage.copied') : t('common:markdownMessage.copy')}
    </button>
  )
}

function MarkdownCode({
  children,
  className,
  node: _node,
  ...props
}: ComponentProps<'code'> & { node?: unknown; 'data-block'?: string }) {
  const isIncomplete = useIsCodeFenceIncomplete()
  if (!Object.hasOwn(props, 'data-block')) {
    return (
      <code className={className || ''} {...props}>
        {children}
      </code>
    )
  }

  const source = textContent(children).replace(/\n$/, '')
  const language = languageName(className)
  return (
    <CodeBlock code={source} isIncomplete={isIncomplete} language={language} lineNumbers={false}>
      <MarkdownCopyButton source={source} />
    </CodeBlock>
  )
}

export type MarkdownMessageProps = {
  children: ReactNode
  className?: string
  streaming?: boolean
}

function MarkdownMessage({ children, className, streaming = false }: MarkdownMessageProps) {
  const source = String(children ?? '')
  return (
    <div
      className={cn('markdown-body', streaming && 'markdown-streaming', className)}
      aria-busy={streaming || undefined}
    >
      <Streamdown
        className="markdown-content space-y-0"
        components={MARKDOWN_COMPONENTS}
        controls={STREAMDOWN_CONTROLS}
        isAnimating={streaming}
        lineNumbers={false}
        mode="streaming"
        plugins={streamdownPlugins}
        remend={STREAMDOWN_REMEND}
      >
        {source}
      </Streamdown>
    </div>
  )
}

const MemoizedMarkdownMessage = memo(MarkdownMessage)
MemoizedMarkdownMessage.displayName = 'MarkdownMessage'

export default MemoizedMarkdownMessage
