import {
  createElement,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ElementType,
  type HTMLAttributes,
  type ReactNode,
} from 'react'
import { cn } from '@/lib/utils'

export type TextTypeRenderState = {
  isTyping: boolean
}

export type TextTypeProps = Omit<HTMLAttributes<HTMLElement>, 'children'> & {
  className?: string
  showCursor?: boolean
  hideCursorWhileTyping?: boolean
  cursorCharacter?: ReactNode
  cursorBlinkDuration?: number
  cursorClassName?: string
  text: string | string[]
  as?: ElementType
  typingSpeed?: number
  initialDelay?: number
  pauseDuration?: number
  deletingSpeed?: number
  loop?: boolean
  textColors?: string[]
  variableSpeed?: { min: number; max: number }
  onSentenceComplete?: (sentence: string, index: number) => void
  startOnVisible?: boolean
  reverseMode?: boolean
  live?: boolean
  controlled?: boolean
  renderText?: (displayedText: string, state: TextTypeRenderState) => ReactNode
}

function prefixLength(previous: string[], next: string[]) {
  const limit = Math.min(previous.length, next.length)
  let index = 0
  while (index < limit && previous[index] === next[index]) index += 1
  return index
}

function liveBatchSize(backlog: number) {
  if (backlog > 180) return Math.min(12, Math.ceil(backlog / 24))
  if (backlog > 72) return 4
  if (backlog > 30) return 2
  return 1
}

export function TextType({
  text,
  as: Component = 'span',
  typingSpeed = 50,
  initialDelay = 0,
  pauseDuration = 2000,
  deletingSpeed = 30,
  loop = true,
  className,
  showCursor = true,
  hideCursorWhileTyping = false,
  cursorCharacter = '|',
  cursorClassName,
  cursorBlinkDuration = 0.5,
  textColors = [],
  variableSpeed,
  onSentenceComplete,
  startOnVisible = false,
  reverseMode = false,
  live = false,
  controlled = false,
  renderText,
  style,
  ...props
}: TextTypeProps) {
  const textArray = useMemo(() => {
    const values = Array.isArray(text) ? text : [text]
    return values.length ? values : ['']
  }, [text])
  const [displayedText, setDisplayedText] = useState('')
  const [currentTextIndex, setCurrentTextIndex] = useState(0)
  const [isDeleting, setIsDeleting] = useState(false)
  const [isVisible, setIsVisible] = useState(!startOnVisible)
  const [reducedMotion, setReducedMotion] = useState(false)
  const containerRef = useRef<HTMLElement | null>(null)
  const previousLiveTargetRef = useRef<string[]>([])
  const completedSentenceRef = useRef('')

  const sourceText = textArray[currentTextIndex % textArray.length] || ''
  const targetText = reverseMode ? Array.from(sourceText).reverse().join('') : sourceText
  const targetCharacters = useMemo(() => Array.from(targetText), [targetText])
  const displayedCharacters = useMemo(() => Array.from(displayedText), [displayedText])
  const visibleText = controlled ? targetText : displayedText
  const isTyping = controlled
    ? live
    : live
      ? displayedCharacters.length < targetCharacters.length
      : isDeleting || displayedCharacters.length < targetCharacters.length

  const getTypingSpeed = useCallback(() => {
    if (!variableSpeed) return typingSpeed
    const minimum = Math.min(variableSpeed.min, variableSpeed.max)
    const maximum = Math.max(variableSpeed.min, variableSpeed.max)
    return minimum + Math.random() * (maximum - minimum)
  }, [typingSpeed, variableSpeed])

  useEffect(() => {
    if (typeof window === 'undefined') return undefined
    const media = window.matchMedia('(prefers-reduced-motion: reduce)')
    const update = () => setReducedMotion(media.matches)
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])

  useEffect(() => {
    if (!startOnVisible) {
      setIsVisible(true)
      return undefined
    }
    const node = containerRef.current
    if (!node || typeof IntersectionObserver === 'undefined') {
      setIsVisible(true)
      return undefined
    }
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (!entry?.isIntersecting) return
        setIsVisible(true)
        observer.disconnect()
      },
      { threshold: 0.1 },
    )
    observer.observe(node)
    return () => observer.disconnect()
  }, [startOnVisible])

  useEffect(() => {
    if (!live || controlled) return
    const previousTarget = previousLiveTargetRef.current
    previousLiveTargetRef.current = targetCharacters
    if (!previousTarget.length || !displayedCharacters.length) return
    const stableLength = prefixLength(previousTarget, targetCharacters)
    if (displayedCharacters.length > stableLength) {
      setDisplayedText(targetCharacters.slice(0, stableLength).join(''))
    }
  }, [controlled, displayedCharacters.length, live, targetCharacters])

  useEffect(() => {
    if (!isVisible || controlled) return undefined
    if (reducedMotion) {
      setDisplayedText(targetText)
      return undefined
    }

    let timeout: ReturnType<typeof setTimeout> | undefined
    if (live) {
      if (displayedCharacters.length >= targetCharacters.length) return undefined
      const backlog = targetCharacters.length - displayedCharacters.length
      const batchSize = liveBatchSize(backlog)
      const delay = displayedCharacters.length === 0 ? initialDelay : getTypingSpeed()
      timeout = setTimeout(
        () => {
          setDisplayedText(
            targetCharacters
              .slice(0, Math.min(targetCharacters.length, displayedCharacters.length + batchSize))
              .join(''),
          )
        },
        Math.max(8, delay),
      )
      return () => clearTimeout(timeout)
    }

    if (isDeleting) {
      if (displayedCharacters.length > 0) {
        timeout = setTimeout(
          () => {
            setDisplayedText(displayedCharacters.slice(0, -1).join(''))
          },
          Math.max(8, deletingSpeed),
        )
      } else {
        setIsDeleting(false)
        setCurrentTextIndex((current) => (current + 1) % textArray.length)
      }
      return () => timeout && clearTimeout(timeout)
    }

    if (displayedCharacters.length < targetCharacters.length) {
      const delay = displayedCharacters.length === 0 ? initialDelay : getTypingSpeed()
      timeout = setTimeout(
        () => {
          setDisplayedText(targetCharacters.slice(0, displayedCharacters.length + 1).join(''))
        },
        Math.max(8, delay),
      )
      return () => clearTimeout(timeout)
    }

    const completionKey = `${currentTextIndex}:${sourceText}`
    if (completedSentenceRef.current !== completionKey) {
      completedSentenceRef.current = completionKey
      onSentenceComplete?.(sourceText, currentTextIndex)
    }
    if (!loop && currentTextIndex === textArray.length - 1) return undefined
    timeout = setTimeout(() => setIsDeleting(true), Math.max(0, pauseDuration))
    return () => clearTimeout(timeout)
  }, [
    controlled,
    currentTextIndex,
    deletingSpeed,
    displayedCharacters,
    getTypingSpeed,
    initialDelay,
    isDeleting,
    isVisible,
    live,
    loop,
    onSentenceComplete,
    pauseDuration,
    reducedMotion,
    sourceText,
    targetCharacters,
    targetText,
    textArray.length,
  ])

  const shouldHideCursor = hideCursorWhileTyping && isTyping
  const content = renderText ? (
    renderText(visibleText, { isTyping })
  ) : (
    <span
      className="rb-text-type__content"
      style={{
        color: textColors.length ? textColors[currentTextIndex % textColors.length] : 'inherit',
      }}
    >
      {visibleText}
    </span>
  )

  return createElement(
    Component,
    {
      ...props,
      ref: containerRef,
      className: cn('rb-text-type', isTyping && 'rb-text-type--typing', className),
      style,
    },
    content,
    showCursor && (
      <span
        aria-hidden="true"
        className={cn(
          'rb-text-type__cursor',
          shouldHideCursor && 'rb-text-type__cursor--hidden',
          cursorClassName,
        )}
        style={{ '--rb-text-type-cursor-duration': `${cursorBlinkDuration}s` } as CSSProperties}
      >
        {cursorCharacter}
      </span>
    ),
  )
}
