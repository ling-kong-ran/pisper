// 斜杠命令菜单：输入 / 时弹出的命令选择面板。内置命令（/new、/model、/run…，与 TUI 对齐）在前，
// 当前目录的 Prompt 模板与 Skill 在后；键盘上下选择 + 回车执行。
import { useEffect, useId, useMemo, useRef, useState, type RefObject } from 'react'
import { Braces, Check, FileText, SquareSlash } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { SessionCommand, SessionCommandsResponse } from './chat-api'
import { matchBuiltinCommands, type BuiltinCommand } from './builtin-commands'
import { commandDraft } from './composer-drafts'
import { AnchoredPopupMenu } from './AnchoredPopupMenu'

type MenuItem =
  { kind: 'builtin'; command: BuiltinCommand } | { kind: 'resource'; command: SessionCommand }

export function ComposerCommandMenu({
  resourceKey,
  loadCommands,
  builtinCommands = [],
  value,
  onChange,
  inputRef,
  placement = 'top',
}: {
  // Prompt / Skill 列表的缓存键（会话 id 或项目目录）：变化时重新向 Runtime 取。
  resourceKey: string
  loadCommands: () => Promise<SessionCommandsResponse>
  builtinCommands?: readonly BuiltinCommand[]
  value: string
  onChange: (value: string) => void
  inputRef: RefObject<HTMLTextAreaElement | null>
  placement?: 'top' | 'bottom'
}) {
  const { t, language } = useI18n()
  const menuId = useId()
  const menuRef = useRef<HTMLDivElement>(null)
  // 单行、以 / 开头才算命令；带空格时只用来补全内置命令的参数（如“/run te”）。
  const typedCommand = value.match(/^\/([^\n]{0,120})$/)
  const [dismissedValue, setDismissedValue] = useState('')
  const [commands, setCommands] = useState<SessionCommand[] | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(false)
  const [activeIndex, setActiveIndex] = useState(0)
  const query = typedCommand?.[1] || ''
  const argumentQuery = /\s/.test(query)
  const loadRef = useRef(loadCommands)
  loadRef.current = loadCommands

  const builtinMatches = useMemo(
    () => matchBuiltinCommands(builtinCommands, query, language),
    [builtinCommands, language, query],
  )
  // 参数补全没有匹配项时不弹空菜单：“/review 某文件”这类带参数的模板照常发送。
  const open = Boolean(
    typedCommand && dismissedValue !== value && (!argumentQuery || builtinMatches.length),
  )

  useEffect(() => {
    setDismissedValue('')
    setCommands(null)
    setError(false)
  }, [resourceKey])

  useEffect(() => {
    if (!open || commands) return
    let active = true
    setLoading(true)
    setError(false)
    loadRef
      .current()
      .then((data) => {
        if (active) setCommands(data.commands || [])
      })
      .catch(() => {
        if (!active) return
        setCommands([])
        setError(true)
      })
      .finally(() => active && setLoading(false))
    return () => {
      active = false
    }
  }, [commands, open, resourceKey])

  const visibleCommands = useMemo<MenuItem[]>(() => {
    const builtins = builtinMatches.map((command) => ({ kind: 'builtin' as const, command }))
    if (argumentQuery) return builtins
    const needle = query.toLocaleLowerCase(language)
    const resources = (commands || []).filter(
      (command) =>
        !needle ||
        `${command.invocation} ${command.name} ${command.description}`
          .toLocaleLowerCase(language)
          .includes(needle),
    )
    return [...builtins, ...resources.map((command) => ({ kind: 'resource' as const, command }))]
  }, [argumentQuery, builtinMatches, commands, language, query])

  useEffect(() => {
    setActiveIndex(0)
  }, [query, commands])

  const close = () => {
    setDismissedValue(value)
  }
  const select = (item: MenuItem) => {
    // 内置叶子命令：清空输入并在本地执行；分组补全成“/verb ”，菜单继续列出子项。
    if (item.kind === 'builtin') {
      const next = item.command.run ? '' : `${item.command.invocation} `
      onChange(next)
      setDismissedValue('')
      setActiveIndex(0)
      if (item.command.run) void item.command.run()
      requestAnimationFrame(() => {
        const input = inputRef.current
        if (!input) return
        input.focus()
        input.setSelectionRange(next.length, next.length)
      })
      return
    }
    const next = commandDraft(item.command.invocation, value)
    onChange(next)
    setDismissedValue(next)
    requestAnimationFrame(() => {
      const input = inputRef.current
      if (!input) return
      input.focus()
      input.setSelectionRange(next.length, next.length)
    })
  }

  useEffect(() => {
    if (!open) return undefined
    const onKeyDown = (event: KeyboardEvent) => {
      if (document.activeElement !== inputRef.current) return
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        close()
        return
      }
      if (!visibleCommands.length) return
      if (event.key === 'ArrowDown') {
        event.preventDefault()
        event.stopPropagation()
        setActiveIndex((current) => (current + 1) % visibleCommands.length)
      } else if (event.key === 'ArrowUp') {
        event.preventDefault()
        event.stopPropagation()
        setActiveIndex((current) => (current - 1 + visibleCommands.length) % visibleCommands.length)
      } else if (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey)) {
        event.preventDefault()
        event.stopPropagation()
        select(visibleCommands[Math.min(activeIndex, visibleCommands.length - 1)])
      }
    }
    document.addEventListener('keydown', onKeyDown, true)
    return () => document.removeEventListener('keydown', onKeyDown, true)
  })

  useEffect(() => {
    const input = inputRef.current
    if (!open || !input) return
    // 菜单 portal 后仍由输入框控制选项，保留键盘焦点与读屏的活动选项关系。
    const attributes = {
      role: 'combobox',
      'aria-autocomplete': 'list',
      'aria-expanded': 'true',
      'aria-controls': menuId,
      'aria-activedescendant': visibleCommands.length
        ? `${menuId}-${Math.min(activeIndex, visibleCommands.length - 1)}`
        : '',
    }
    const previous = Object.keys(attributes).map((name) => [name, input.getAttribute(name)])
    for (const [name, value] of Object.entries(attributes)) {
      if (value) input.setAttribute(name, value)
      else input.removeAttribute(name)
    }
    return () => {
      for (const [name, value] of previous) {
        if (!name) continue
        if (value === null) input.removeAttribute(name)
        else input.setAttribute(name, value)
      }
    }
  }, [activeIndex, inputRef, menuId, open, visibleCommands.length])

  const scopeLabels = {
    user: t('chat:commands.scope.user'),
    project: t('chat:commands.scope.project'),
    package: t('chat:commands.scope.package'),
    custom: t('chat:commands.scope.custom'),
  }

  return (
    <AnchoredPopupMenu
      open={open}
      anchorRef={inputRef}
      menuRef={menuRef}
      placement={placement}
      matchAnchorWidth
      maxHeight={330}
      id={menuId}
      role="listbox"
      ariaLabel={t('chat:commands.open')}
      className="permission-mode-menu composer-command-menu overflow-y-auto rounded-md bg-solid p-1 text-content shadow-float"
    >
      <div className="chat-resource-list composer-command-list flex min-h-0 [flex:1_1_0] flex-col gap-px overflow-y-auto [overscroll-behavior:contain] [scrollbar-gutter:stable] [touch-action:pan-y] [&_>_button]:grid [&_>_button]:w-full [&_>_button]:cursor-pointer [&_>_button]:grid-cols-[28px_minmax(0,1fr)_auto] [&_>_button]:items-center [&_>_button]:gap-2 [&_>_button]:rounded-sm [&_>_button]:px-1.5 [&_>_button]:py-1 [&_>_button]:text-left [&_>_button]:text-content [&_>_button.active]:bg-surface-hover max-[650px]:[&_>_button]:min-h-11 [&_button_>_span:nth-child(2)]:flex [&_button_>_span:nth-child(2)]:min-w-0 [&_button_>_span:nth-child(2)]:flex-col [&_strong]:truncate [&_strong]:font-mono [&_strong]:text-[13px] [&_strong]:font-medium [&_small]:truncate [&_small]:font-sans [&_small]:text-xs [&_small]:font-normal [&_small]:text-content-muted [&_em]:text-xs [&_em]:whitespace-nowrap [&_em]:text-content-muted [&_em]:not-italic [&_>_p]:m-auto [&_>_p]:px-2 [&_>_p]:py-1.5 [&_>_p]:text-xs [&_>_p]:text-content-muted">
        {visibleCommands.map((item, index) => {
          const { command } = item
          const Icon =
            item.kind === 'builtin'
              ? item.command.current
                ? Check
                : SquareSlash
              : item.command.source === 'prompt'
                ? FileText
                : Braces
          const resource = item.kind === 'resource' ? item.command : null
          return (
            <button
              type="button"
              role="option"
              id={`${menuId}-${index}`}
              aria-selected={index === activeIndex}
              className={index === activeIndex ? 'active' : ''}
              key={`${item.kind}:${command.invocation}`}
              onMouseEnter={() => setActiveIndex(index)}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => select(item)}
            >
              <span className="list-icon grid size-7 place-items-center rounded-sm bg-surface-muted text-content-secondary">
                <Icon size={15} />
              </span>
              <span className="composer-command-copy">
                <strong>
                  {command.invocation}
                  {resource?.argumentHint && <small> {resource.argumentHint}</small>}
                </strong>
                <small>{command.description || t('chat:commands.noDescription')}</small>
              </span>
              <em>
                {resource ? (
                  <>
                    {resource.source === 'prompt'
                      ? t('chat:commands.prompt')
                      : t('chat:commands.skill')}
                    {' · '}
                    {scopeLabels[resource.scope]}
                  </>
                ) : item.kind === 'builtin' && item.command.current ? (
                  t('chat:commands.builtin.current')
                ) : (
                  t('chat:commands.builtin.label')
                )}
              </em>
            </button>
          )
        })}
        {!visibleCommands.length && (
          <p>
            {loading
              ? t('chat:commands.loading')
              : error
                ? t('chat:commands.loadError')
                : t('chat:commands.empty')}
          </p>
        )}
      </div>
    </AnchoredPopupMenu>
  )
}
