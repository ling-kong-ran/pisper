import type { CSSProperties } from 'react'
import { cn } from '@/lib/utils'

export function BlurText({
  text,
  className,
  delay = 45,
}: {
  text: string
  className?: string
  delay?: number
}) {
  const words = text.split(/(\s+)/)
  return (
    <span className={cn('rb-blur-text', className)} aria-label={text}>
      {words.map((word, index) =>
        /^\s+$/.test(word) ? (
          word
        ) : (
          <span
            aria-hidden="true"
            className="rb-blur-text-word"
            key={`${word}-${index}`}
            style={{ '--rb-delay': `${index * delay}ms` } as CSSProperties}
          >
            {word}
          </span>
        ),
      )}
    </span>
  )
}
