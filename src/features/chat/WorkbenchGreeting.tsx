// 视觉改编自 ZCode ConversationDraftEmptyState；来源和改动见 THIRD_PARTY_NOTICES.md。
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import darkLogo from '@/assets/zcode-empty-dark.svg'
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
        className="pointer-events-none absolute top-1/2 left-1/2 -mt-10 aspect-[5/4] w-[min(72vw,25rem)] -translate-x-1/2 -translate-y-1/2 text-foreground/40"
      >
        <svg
          className="h-full w-full opacity-70 dark:hidden [mask-image:linear-gradient(to_bottom,black_0%,transparent_70%)] [-webkit-mask-image:linear-gradient(to_bottom,black_0%,transparent_70%)]"
          width="400"
          height="320"
          viewBox="0 0 400 320"
          fill="none"
        >
          <path
            d="M398.97 0.5L147.576 319.5H1.03027L37.5996 273.081L120.167 169.603L120.171 169.598L215.342 47.5605L215.343 47.5615L252.424 0.5H398.97ZM264.544 273.271H372.527L336.082 319.498H189.886L202.642 303.307C217.584 284.34 240.398 273.271 264.544 273.271ZM209.164 0.5L202.786 8.58887C183.782 32.6885 154.782 46.752 124.091 46.752H25.9805L62.4268 0.5H209.164Z"
            stroke="currentColor"
          />
        </svg>
        <img className="hidden h-full w-full dark:block" src={darkLogo} alt="" />
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
