// 斜杠命令菜单：输入 / 时弹出的命令选择面板（新会话/切换模型等），
// 键盘上下选择 + 回车执行。
import { useEffect, useId, useMemo, useState, type RefObject } from 'react'
import { Braces, FileText } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { chatApi, type SessionCommand } from './chat-api'

export function commandDraft(invocation: string, value: string) {
  const slash = value.match(/^\/[^\s]*(?:\s+([\s\S]*))?$/)
  const argumentsText = (slash ? slash[1] || '' : value).trim()
  return `${invocation}${argumentsText ? ` ${argumentsText}` : ' '}`
}

export function ComposerCommandMenu({
  sessionId,
  value,
  onChange,
  inputRef,
}: {
  sessionId: string
  value: string
  onChange: (value: string) => void
  inputRef: RefObject<HTMLTextAreaElement | null>
}) {
  const { t, language } = useI18n()
  const menuId = useId()
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

  const scopeLabels = {
    user: t('chat:commands.scope.user'),
    project: t('chat:commands.scope.project'),
    package: t('chat:commands.scope.package'),
    custom: t('chat:commands.scope.custom'),
  }

  return (
    <>
      {open && (
        <div
          className="permission-mode-menu [&_>_button]:grid [&_>_button]:w-full [&_>_button]:min-h-[48px] [&_>_button]:grid-cols-[auto_minmax(0,1fr)_auto] [&_>_button]:items-center [&_>_button]:gap-[8px] [&_>_button]:border-0 [&_>_button]:rounded-[var(--r-sm)] [&_>_button]:bg-transparent [&_>_button]:text-[var(--text)] [&_>_button]:p-[6px_7px] [&_>_button]:text-left [&_>_button:hover]:bg-[var(--accent-soft)] [&_>_button.active]:bg-[var(--accent-soft)] [&_>_button_>_span:nth-child(2)]:flex [&_>_button_>_span:nth-child(2)]:min-w-0 [&_>_button_>_span:nth-child(2)]:flex-col [&_>_button_>_span:nth-child(2)]:gap-[2px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_small]:leading-[1.4] [&_>_button_>_svg]:text-[var(--star-strong)] max-[650px]:[.focus-composer_&]:right-[auto] max-[650px]:[.focus-composer_&]:left-0 max-[650px]:[.focus-composer_&]:w-[min(250px,calc(100vw_-_76px))] absolute z-[35] right-0 [bottom:calc(100%_+_8px)] w-[250px] overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--solid)] [padding:5px] shadow-[0_18px_42px_-18px_var(--menu-shadow)] composer-command-menu"
          id={menuId}
          role="listbox"
          aria-label={t('chat:commands.open')}
          style={{
            right: 8,
            left: 8,
            width: 'auto',
            maxHeight: 'min(330px, 48dvh)',
            overflowY: 'auto',
            padding: 6,
          }}
        >
          <div
            className="chat-resource-list [&_>_button]:grid [&_>_button]:w-full [&_>_button]:grid-cols-[30px_minmax(0,1fr)_auto] [&_>_button]:items-center [&_>_button]:gap-[8px] [&_>_button]:[border:1px_solid_transparent] [&_>_button]:rounded-[var(--r-sm)] [&_>_button]:bg-transparent [&_>_button]:p-[8px] [&_>_button]:text-[var(--text)] [&_>_button]:text-left [&_>_button]:cursor-pointer [&_>_button:hover]:bg-[var(--surface-hover)] [&_>_button.active]:border-[var(--focus)] [&_>_button.active]:bg-[var(--blue-soft)] [&_button_>_span:nth-child(2)]:flex [&_button_>_span:nth-child(2)]:min-w-0 [&_button_>_span:nth-child(2)]:flex-col [&_button_>_span:nth-child(2)]:gap-[2px] [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_em]:text-[var(--text-muted)] [&_em]:text-[10px] [&_em]:[font-style:normal] [&_>_p]:m-[auto] [&_>_p]:text-[var(--text-muted)] [&_>_p]:text-[12px] flex min-h-0 [flex:1_1_0] flex-col gap-[3px] overflow-y-auto [overscroll-behavior:contain] [scrollbar-gutter:stable] [touch-action:pan-y] [margin-top:9px] composer-command-list"
            style={{ marginTop: 0 }}
          >
            {visibleCommands.map((command, index) => {
              const Icon = command.source === 'prompt' ? FileText : Braces
              return (
                <button
                  type="button"
                  role="option"
                  aria-selected={index === activeIndex}
                  className={index === activeIndex ? 'active' : ''}
                  key={command.invocation}
                  onMouseEnter={() => setActiveIndex(index)}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => select(command)}
                >
                  <span className="list-icon [.chat-resource-list_&]:grid [.chat-resource-list_&]:w-[28px] [.chat-resource-list_&]:h-[28px] [.chat-resource-list_&]:place-items-center [.chat-resource-list_&]:rounded-[var(--r-sm)] [.chat-resource-list_&]:bg-[var(--surface-subtle)] [.chat-resource-list_&]:text-[var(--star-strong)] [.session-workflow-summary_&]:grid [.session-workflow-summary_&]:w-[28px] [.session-workflow-summary_&]:h-[28px] [.session-workflow-summary_&]:place-items-center [.session-workflow-summary_&]:rounded-[var(--r-sm)] [.session-workflow-summary_&]:bg-[var(--surface-subtle)] [.session-workflow-summary_&]:text-[var(--star-strong)] grid w-[27px] h-[27px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)] [.workflow-template-gallery_&]:grid [.workflow-template-gallery_&]:w-[32px] [.workflow-template-gallery_&]:h-[32px] [.workflow-template-gallery_&]:place-items-center [.workflow-template-gallery_&]:rounded-[var(--r-sm)] [.workflow-template-gallery_&]:bg-[var(--surface-subtle)] [.workflow-template-gallery_&]:text-[var(--star-strong)]">
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
                    {command.source === 'prompt'
                      ? t('chat:commands.prompt')
                      : t('chat:commands.skill')}
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
        </div>
      )}
    </>
  )
}
