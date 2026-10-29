// 斜杠命令菜单：输入 / 时弹出的命令选择面板（新会话/切换模型等），
// 键盘上下选择 + 回车执行。
import { useEffect, useId, useMemo, useRef, useState, type RefObject } from 'react'
import { Braces, FileText } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { chatApi, type SessionCommand } from './chat-api'
import { commandDraft } from './composer-drafts'
import { AnchoredPopupMenu } from './AnchoredPopupMenu'

export function ComposerCommandMenu({
  sessionId,
  value,
  onChange,
  inputRef,
  placement = 'top',
}: {
  sessionId: string
  value: string
  onChange: (value: string) => void
  inputRef: RefObject<HTMLTextAreaElement | null>
  placement?: 'top' | 'bottom'
}) {
  const { t, language } = useI18n()
  const menuId = useId()
  const menuRef = useRef<HTMLDivElement>(null)
  const typedCommand = value.match(/^\/([^\s]*)$/)
  const [dismissedValue, setDismissedValue] = useState('')
  const [commands, setCommands] = useState<SessionCommand[] | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(false)
  const [activeIndex, setActiveIndex] = useState(0)
  const open = Boolean(typedCommand && dismissedValue !== value)
  const query = typedCommand?.[1] || ''

  useEffect(() => {
    setDismissedValue('')
    setCommands(null)
    setError(false)
  }, [sessionId])

  useEffect(() => {
    if (!open || commands) return
    let active = true
    setLoading(true)
    setError(false)
    chatApi
      .getSessionCommands(sessionId)
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
  }, [commands, open, sessionId])

  const visibleCommands = useMemo(() => {
    const needle = query.toLocaleLowerCase(language)
    if (!needle) return commands || []
    return (commands || []).filter((command) =>
      `${command.invocation} ${command.name} ${command.description}`
        .toLocaleLowerCase(language)
        .includes(needle),
    )
  }, [commands, language, query])

  useEffect(() => {
    setActiveIndex(0)
  }, [query, commands])

  const close = () => {
    setDismissedValue(value)
  }
  const select = (command: SessionCommand) => {
    const next = commandDraft(command.invocation, value)
    onChange(next)
    setDismissedValue(next)
    requestAnimationFrame(() => {
      const input = inputRef.current
      if (!input) return
      input.focus()
      input.setSelectionRange(next.length, next.length)
      input.style.height = 'auto'
      input.style.height = `${Math.min(input.scrollHeight, 220)}px`
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
        {visibleCommands.map((command, index) => {
          const Icon = command.source === 'prompt' ? FileText : Braces
          return (
            <button
              type="button"
              role="option"
              id={`${menuId}-${index}`}
              aria-selected={index === activeIndex}
              className={index === activeIndex ? 'active' : ''}
              key={command.invocation}
              onMouseEnter={() => setActiveIndex(index)}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => select(command)}
            >
              <span className="list-icon grid size-7 place-items-center rounded-sm bg-surface-muted text-content-secondary">
                <Icon size={15} />
              </span>
              <span className="composer-command-copy">
                <strong>
                  {command.invocation}
                  {command.argumentHint && <small> {command.argumentHint}</small>}
                </strong>
                <small>{command.description || t('chat:commands.noDescription')}</small>
              </span>
              <em>
                {command.source === 'prompt' ? t('chat:commands.prompt') : t('chat:commands.skill')}
                {' · '}
                {scopeLabels[command.scope]}
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
