// Markdown 渲染组件：基于 streamdown 的流式解析 + 自定义组件映射
// （代码块/链接/图片/行内引用），代码块提供复制按钮。
// 增量 block 解析器只在末尾追加时重解析尾部，配合打字机效果保持流畅。
import { isValidElement, memo, useState, type ComponentProps, type ReactNode } from 'react'
import { Check, Copy } from 'lucide-react'
import { CodeBlock, Streamdown, useIsCodeFenceIncomplete, type Components } from 'streamdown'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { createIncrementalBlockParser, streamdownPlugins } from '@/lib/streamdown'
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
    <Button
      type="button"
      variant="ghost"
      size="sm"
      className="h-7 gap-1 px-[7px] text-[11px] font-bold text-[var(--code-toolbar-text)] hover:bg-white/10 hover:text-[var(--code-toolbar-text)]"
      data-streamdown="code-block-copy-button"
      onClick={copy}
      aria-label={t('common:markdownMessage.copyCode')}
      title={t('common:markdownMessage.copyCode')}
    >
      {copied ? <Check size={13} /> : <Copy size={13} />}
      {copied ? t('common:markdownMessage.copied') : t('common:markdownMessage.copy')}
    </Button>
  )
}

/**
 * 流式期间未闭合代码块的轻量渲染：结构与 streamdown CodeBlock 一致，
 * 但只做纯文本展示。这样 typewriter 每次刷新（约 48ms）不会触发 shiki
 * 高亮——增长中的代码每变一次都会产生一份 token 结果，开销和驻留内存
 * 都很可观。围栏闭合（isIncomplete 变为 false）后再挂载 CodeBlock 高亮。
 */
function StreamingCodeBlock({ language, source }: { language: string; source: string }) {
  return (
    <div
      className="my-4 flex w-full flex-col gap-2 rounded-xl border border-border bg-sidebar p-2"
      data-incomplete="true"
      data-language={language}
      data-streamdown="code-block"
      style={{ contentVisibility: 'auto', containIntrinsicSize: 'auto 200px' }}
    >
      <div
        className="flex h-8 items-center justify-between text-muted-foreground text-xs"
        data-language={language}
        data-streamdown="code-block-header"
      >
        <span className="ml-1 font-mono lowercase">{language}</span>
        <MarkdownCopyButton source={source} />
      </div>
      <div
        className="overflow-x-auto rounded-md border border-border bg-background p-4 text-sm"
        data-language={language}
        data-streamdown="code-block-body"
      >
        <pre className="bg-transparent">
          <code>{source}</code>
        </pre>
      </div>
    </div>
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
  if (isIncomplete) return <StreamingCodeBlock language={language} source={source} />
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
  const [parseBlocks] = useState(() => createIncrementalBlockParser())
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
        parseMarkdownIntoBlocksFn={parseBlocks}
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
