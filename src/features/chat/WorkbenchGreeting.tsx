// 视觉改编自 ZCode ConversationDraftEmptyState；来源和改动见 THIRD_PARTY_NOTICES.md。
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import {
  greetingFontSize,
  greetingPeriod,
  nextGreetingDelay,
} from '@/features/chat/workbench-greeting'

export function WorkbenchGreeting() {
  const { t } = useI18n()
  const [date, setDate] = useState(() => new Date())
  const [fontSize, setFontSize] = useState(30)
  const container = useRef<HTMLHeadingElement>(null)
  const measurement = useRef<HTMLSpanElement>(null)
  const greeting = [
    t('chat:workbench.morningEarly'),
    t('chat:workbench.morning'),
    t('chat:workbench.noon'),
    t('chat:workbench.afternoon'),
    t('chat:workbench.evening'),
    t('chat:workbench.lateNight'),
  ][greetingPeriod(date)]
  useEffect(() => {
    const timer = window.setTimeout(() => setDate(new Date()), nextGreetingDelay(date))
    const refresh = () => setDate(new Date())
    window.addEventListener('focus', refresh)
    return () => {
      window.clearTimeout(timer)
      window.removeEventListener('focus', refresh)
    }
  }, [date])
  useLayoutEffect(() => {
    const heading = container.current
    const span = measurement.current
    if (!heading || !span) return
    const measure = () =>
      setFontSize(greetingFontSize(heading.clientWidth - 32, span.getBoundingClientRect().width))
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(heading)
    observer.observe(span)
    return () => observer.disconnect()
  }, [greeting])
  return (
    <div
      className="relative mx-auto translate-y-10 max-[650px]:translate-y-0 flex w-full max-w-2xl items-center justify-center text-foreground [[data-mobile-keyboard='open']_&]:pointer-events-none [[data-mobile-keyboard-transition='opening']_&]:pointer-events-none"
      data-testid="workbench-greeting"
    >
      <div
        aria-hidden="true"
        className="pointer-events-none absolute top-1/2 left-1/2 -mt-10 max-[650px]:mt-0 aspect-[5/4] w-[min(72vw,25rem)] -translate-x-1/2 -translate-y-1/2 text-foreground/40"
      >
        <svg
          data-brand="Pisper"
          className="h-full w-full opacity-70 [mask-image:linear-gradient(to_bottom,black_0%,transparent_70%)] [-webkit-mask-image:linear-gradient(to_bottom,black_0%,transparent_70%)]"
          width="400"
          height="320"
          viewBox="0 0 400 320"
          fill="none"
        >
          <path
            d="M88 319V1h135c76 0 121 43 121 110s-45 111-121 111h-76v97H88Zm59-153h72c43 0 65-19 65-55s-22-54-65-54h-72v109Z"
            stroke="currentColor"
          />
        </svg>
      </div>
      <h1
        ref={container}
        style={{ fontSize }}
        className="relative z-10 m-0 w-full px-4 text-center leading-[1.2] font-medium"
      >
        <span
          ref={measurement}
          aria-hidden="true"
          className="pointer-events-none invisible absolute text-3xl leading-[1.2] whitespace-nowrap"
        >
          {greeting}
        </span>
        <span>{greeting}</span>
      </h1>
    </div>
  )
}
