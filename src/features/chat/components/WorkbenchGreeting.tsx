// 视觉改编自 ZCode ConversationDraftEmptyState；来源和改动见 THIRD_PARTY_NOTICES.md。
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { BrandLogo } from '@/components/common/BrandLogo'
import { useI18n } from '@/app/i18n/use-i18n'
import {
  greetingFontSize,
  greetingPeriod,
  nextGreetingDelay,
} from '@/features/chat/model/workbench-greeting'

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
      className="relative mx-auto flex w-full max-w-2xl flex-col items-center justify-center gap-4 text-foreground [[data-mobile-keyboard='open']_&]:pointer-events-none [[data-mobile-keyboard-transition='opening']_&]:pointer-events-none"
      data-testid="workbench-greeting"
    >
      <div aria-hidden="true">
        <BrandLogo size={44} />
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
