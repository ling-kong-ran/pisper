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
          className="permission-mode-menu composer-command-menu"
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
          <div className="chat-resource-list composer-command-list" style={{ marginTop: 0 }}>
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
                  <span className="list-icon">
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
