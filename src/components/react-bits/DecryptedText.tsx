import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ComponentPropsWithoutRef,
} from 'react'
import { cn } from '@/lib/utils'

type DecryptedTextProps = Omit<ComponentPropsWithoutRef<'span'>, 'children' | 'className'> & {
  text: string
  speed?: number
  maxIterations?: number
  sequential?: boolean
  revealDirection?: 'start' | 'end' | 'center'
  useOriginalCharsOnly?: boolean
  characters?: string
  className?: string
  parentClassName?: string
  encryptedClassName?: string
  animateOn?: 'view' | 'hover' | 'change'
}

type AnimatedCharacter = {
  character: string
  revealed: boolean
}

type DecryptedFrame = {
  prefix: string
  suffix: AnimatedCharacter[]
  animating: boolean
}

const DEFAULT_CHARACTERS =
  'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*()_+-=[]{};:,.<>/?'

function commonPrefixLength(previous: string[], next: string[]) {
  const limit = Math.min(previous.length, next.length)
  let index = 0
  while (index < limit && previous[index] === next[index]) index += 1
  return index
}

function revealOrder(length: number, direction: DecryptedTextProps['revealDirection']) {
  if (direction === 'end') return Array.from({ length }, (_, index) => length - 1 - index)
  if (direction === 'center') {
    const middle = (length - 1) / 2
    return Array.from({ length }, (_, index) => index).sort(
      (left, right) => Math.abs(left - middle) - Math.abs(right - middle),
    )
  }
  return Array.from({ length }, (_, index) => index)
}

function shuffled(values: number[]) {
  const result = [...values]
  for (let index = result.length - 1; index > 0; index -= 1) {
    const next = Math.floor(Math.random() * (index + 1))
    ;[result[index], result[next]] = [result[next], result[index]]
  }
  return result
}

function isWhitespace(character: string) {
  return /^\s$/u.test(character)
}

export const DecryptedText = memo(function DecryptedText({
  text,
  speed = 28,
  maxIterations = 4,
  sequential = false,
  revealDirection = 'start',
  useOriginalCharsOnly = false,
  characters = DEFAULT_CHARACTERS,
  className,
  parentClassName,
  encryptedClassName,
  animateOn = 'view',
  onMouseEnter,
  onMouseLeave,
  ...props
}: DecryptedTextProps) {
  const [frame, setFrame] = useState<DecryptedFrame>({
    prefix: text,
    suffix: [],
    animating: false,
  })
  const containerRef = useRef<HTMLSpanElement>(null)
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const previousTextRef = useRef(animateOn === 'change' ? '' : text)
  const hasAnimatedRef = useRef(false)

  const availableCharacters = useMemo(() => {
    const source = useOriginalCharsOnly
      ? Array.from(new Set(Array.from(text).filter((character) => !isWhitespace(character))))
      : Array.from(characters)
    return source.length ? source : Array.from(DEFAULT_CHARACTERS)
  }, [characters, text, useOriginalCharsOnly])

  const stopAnimation = useCallback(() => {
    if (intervalRef.current) clearInterval(intervalRef.current)
    intervalRef.current = null
  }, [])

  const showPlainText = useCallback(() => {
    stopAnimation()
    setFrame({ prefix: text, suffix: [], animating: false })
  }, [stopAnimation, text])

  const startAnimation = useCallback(
    (prefixLength = 0) => {
      stopAnimation()
      const target = Array.from(text)
      if (!target.length) {
        setFrame({ prefix: '', suffix: [], animating: false })
        return
      }
      if (
        typeof window === 'undefined' ||
        window.matchMedia('(prefers-reduced-motion: reduce)').matches
      ) {
        setFrame({ prefix: text, suffix: [], animating: false })
        return
      }

      const safePrefixLength = Math.min(Math.max(0, prefixLength), target.length)
      const prefix = target.slice(0, safePrefixLength).join('')
      const suffix = target.slice(safePrefixLength)
      const directionalOrder = revealOrder(suffix.length, revealDirection).filter(
        (index) => !isWhitespace(suffix[index] || ''),
      )
      const order = sequential ? directionalOrder : shuffled(directionalOrder)
      const iterations = Math.max(1, Math.floor(maxIterations))
      let iteration = 0

      const paint = (revealedCount: number, animating: boolean) => {
        const revealed = new Set(order.slice(0, revealedCount))
        setFrame({
          prefix,
          animating,
          suffix: suffix.map((character, index) => {
            const isRevealed = isWhitespace(character) || revealed.has(index)
            return {
              character: isRevealed
                ? character
                : availableCharacters[Math.floor(Math.random() * availableCharacters.length)] ||
                  character,
              revealed: isRevealed,
            }
          }),
        })
      }

      paint(0, true)
      intervalRef.current = setInterval(
        () => {
          iteration += 1
          const revealedCount = Math.ceil((order.length * iteration) / iterations)
          if (iteration >= iterations) {
            stopAnimation()
            setFrame({ prefix: text, suffix: [], animating: false })
            return
          }
          paint(revealedCount, true)
        },
        Math.max(8, speed),
      )
    },
    [availableCharacters, maxIterations, revealDirection, sequential, speed, stopAnimation, text],
  )

  useEffect(() => {
    if (animateOn !== 'change') {
      previousTextRef.current = text
      showPlainText()
      return
    }
    const previous = Array.from(previousTextRef.current)
    const next = Array.from(text)
    previousTextRef.current = text
    startAnimation(commonPrefixLength(previous, next))
  }, [animateOn, showPlainText, startAnimation, text])

  useEffect(() => {
    if (animateOn !== 'view') return undefined
    const node = containerRef.current
    if (!node || typeof IntersectionObserver === 'undefined') {
      startAnimation(0)
      return undefined
    }
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (!entry?.isIntersecting || hasAnimatedRef.current) return
        hasAnimatedRef.current = true
        startAnimation(0)
        observer.disconnect()
      },
      { threshold: 0.1 },
    )
    observer.observe(node)
    return () => observer.disconnect()
  }, [animateOn, startAnimation])

  useEffect(() => () => stopAnimation(), [stopAnimation])

  return (
    <span
      {...props}
      ref={containerRef}
      className={cn('rb-decrypted-text', frame.animating && 'is-animating', parentClassName)}
      onMouseEnter={(event) => {
        onMouseEnter?.(event)
        if (animateOn === 'hover') startAnimation(0)
      }}
      onMouseLeave={(event) => {
        onMouseLeave?.(event)
        if (animateOn === 'hover') showPlainText()
      }}
    >
      <span className="rb-decrypted-text__sr">{text}</span>
      <span aria-hidden="true">
        {frame.prefix}
        {frame.suffix.map((item, index) => (
          <span
            className={item.revealed ? className : cn(className, encryptedClassName)}
            key={index}
          >
            {item.character}
          </span>
        ))}
      </span>
    </span>
  )
})
