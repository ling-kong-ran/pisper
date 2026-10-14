// Markdown 渲染组件：基于 streamdown 的流式解析 + 自定义组件映射
// （代码块/链接/图片/行内引用），代码块提供复制按钮。
// 增量 block 解析器只在末尾追加时重解析尾部，配合打字机效果保持流畅。
import {
  isValidElement,
  memo,
  useEffect,
  useMemo,
  useState,
  type ComponentProps,
  type ReactNode,
} from 'react'
import { Check, Copy } from 'lucide-react'
import {
  CodeBlock,
  Streamdown,
  defaultRemarkPlugins,
  useIsCodeFenceIncomplete,
  type Components,
} from 'streamdown'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  createIncrementalBlockParser,
  parseMarkdownBlocksCached,
  streamdownPlugins,
} from '@/lib/streamdown'
import { loadKatexStyles, looksLikeMath } from '@/lib/katex-styles'
import { decodeLocalFileHref, remarkLocalFileLinks } from '@/lib/local-file-links'
import {
  LocalPathRevealError,
  requestLocalPathReveal,
  revealPathThroughRuntime,
} from '@/lib/local-path-reveal'
import { LOCAL_REVEAL_NOTICE_EVENT } from '@/app/route-context'
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

// 本地路径 reveal 结果交给应用壳的统一 Toast 展示（App.tsx 监听）。
function emitLocalRevealNotice(message: string, tone: 'info' | 'error') {
  window.dispatchEvent(new CustomEvent(LOCAL_REVEAL_NOTICE_EVENT, { detail: { message, tone } }))
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

function MarkdownLink({
  children: label,
  node: _node,
  className,
  href,
  ...props
}: ComponentProps<'a'> & { node?: unknown }) {
  const { t } = useI18n()
  const [revealPending, setRevealPending] = useState(false)
  const [revealFailure, setRevealFailure] = useState('')
  const content = textContent(label).trim() ? label : href
  if (!href) return <span className={cn('markdown-link', className)}>{content}</span>

  const localFile = decodeLocalFileHref(href)
  if (localFile) {
    const title = t('common:markdownMessage.revealLocalPath', { path: localFile.path })
    const reveal = async () => {
      setRevealPending(true)
      setRevealFailure('')
      try {
        // 点击时再请求 Runtime，桌面 WebView 与普通 Web 页面使用同一条 Node 链路。
        await requestLocalPathReveal(localFile.path, revealPathThroughRuntime)
        emitLocalRevealNotice(
          t('common:markdownMessage.revealLocalPathOk', { path: localFile.path }),
          'info',
        )
      } catch (error: unknown) {
        const message =
          error instanceof LocalPathRevealError && error.reason === 'unavailable'
            ? t('common:markdownMessage.revealLocalPathUnavailable')
            : error instanceof LocalPathRevealError && error.reason === 'timeout'
              ? t('common:markdownMessage.revealLocalPathTimeout')
              : t('common:markdownMessage.revealLocalPathFailed', { path: localFile.path })
        const detail = error instanceof Error ? error.message : String(error)
        console.error('[markdown] 无法在文件管理器中显示本地路径。', error)
        // 错误留在链接旁：即使 Toast 监听尚未挂载，也能看到原因并复制路径自助定位。
        setRevealFailure(`${message} (${detail})`)
        emitLocalRevealNotice(message, 'error')
      } finally {
        setRevealPending(false)
      }
    }
    return (
      <>
        <button
          type="button"
          className={cn(
            'markdown-link inline cursor-pointer border-0 bg-transparent p-0 [font:inherit]',
            className,
          )}
          data-local-path={localFile.path}
          title={title}
          disabled={revealPending}
          aria-busy={revealPending || undefined}
          onClick={() => void reveal()}
        >
          {content}
        </button>
        {revealFailure && (
          <span role="alert" className="ml-2 text-sm text-destructive">
            {revealFailure}{' '}
            <button
              type="button"
              className="cursor-pointer underline underline-offset-2"
              onClick={() => {
                void copyText(localFile.path).then(
                  () => emitLocalRevealNotice(t('common:markdownMessage.copied'), 'info'),
                  () => emitLocalRevealNotice(t('common:markdownMessage.copyPathFailed'), 'error'),
                )
              }}
            >
              {t('common:markdownMessage.copyPath')}
            </button>
          </span>
        )}
      </>
    )
  }

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
  /** 会话工作区根目录：供本地文件链接插件解析相对路径（如 workspace/报告.docx）。 */
  cwd?: string
}

function MarkdownMessage({ children, className, streaming = false, cwd }: MarkdownMessageProps) {
  const source = String(children ?? '')
  // 本地文件链接哨兵插件按工作区基址构建：相对路径链接只有带上 cwd 才能解析，
  // 因此不能做成模块级单例；以 [plugin, options] 形式注册，cwd 稳定时 useMemo 保证引用不变。
  const remarkPlugins = useMemo(
    () => [
      ...Object.values(defaultRemarkPlugins),
      // unified 要求 [plugin, options] 元组形式注册，不能直接传实例化的 transformer。
      [remarkLocalFileLinks, cwd || undefined] as [typeof remarkLocalFileLinks, string | undefined],
    ],
    [cwd],
  )
  // 公式样式按需注入：命中数学语法才加载 KaTeX CSS，避免主 CSS 常驻。
  const hasMath = useMemo(() => looksLikeMath(source), [source])
  useEffect(() => {
    if (hasMath) void loadKatexStyles()
  }, [hasMath])
  const [parseBlocks] = useState(() => createIncrementalBlockParser())
  // 流式消息用实例级增量解析器（追加路径近零成本）；完成后文本恒定，
  // 走全局 LRU——虚拟化重挂载不再每次全量重解析。
  const parseBlocksFn = streaming ? parseBlocks : parseMarkdownBlocksCached
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
        parseMarkdownIntoBlocksFn={parseBlocksFn}
        plugins={streamdownPlugins}
        remarkPlugins={remarkPlugins}
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
