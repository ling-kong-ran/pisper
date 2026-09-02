// 欢迎页动效:空会话时的品牌舞台、轮换标题与入场动效。
import { useEffect, useState, type ReactNode } from 'react'
import { BlurText } from '@/components/react-bits/BlurText'
import { TargetCursor } from '@/components/react-bits/TargetCursor'
import { WelcomeBrandStage } from './welcome-brand'

type WelcomeEffectsProps = {
  children: ReactNode
  titles: string[]
}

const ROTATE_MS = 60 * 60 * 1000
const FADE_MS = 320

// 轮换标题:先整体淡出(透明度+模糊),换词后靠 BlurText 重挂载播入场。
// 减弱动态时冻结在首句,不做任何定时切换。
// 文档隐藏时挂起计时并记住剩余时间,回前台后从剩余处继续,避免后台空转触发重渲染。
function useRotatingTitle(titles: string[]) {
  const [index, setIndex] = useState(0)
  const [leaving, setLeaving] = useState(false)
  useEffect(() => {
    if (titles.length < 2) return
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return
    let rotateTimer = 0
    let fadeTimer = 0
    let startedAt = 0
    let remaining = ROTATE_MS
    const schedule = () => {
      startedAt = Date.now()
      rotateTimer = window.setTimeout(() => {
        setLeaving(true)
        fadeTimer = window.setTimeout(() => {
          setIndex((current) => (current + 1) % titles.length)
          setLeaving(false)
        }, FADE_MS)
        remaining = ROTATE_MS
        schedule()
      }, remaining)
    }
    const syncWithVisibility = () => {
      if (document.hidden) {
        // 扣除已流逝的时间后挂起,保证恢复后按剩余时长继续而不是重新计满一轮
        remaining = Math.max(0, remaining - (Date.now() - startedAt))
        window.clearTimeout(rotateTimer)
      } else {
        schedule()
      }
    }
    schedule()
    document.addEventListener('visibilitychange', syncWithVisibility)
    return () => {
      window.clearTimeout(rotateTimer)
      window.clearTimeout(fadeTimer)
      document.removeEventListener('visibilitychange', syncWithVisibility)
    }
  }, [titles.length])
  return { title: titles[index] || titles[0] || '', leaving }
}

export default function WelcomeEffects({ children, titles }: WelcomeEffectsProps) {
  const { title, leaving } = useRotatingTitle(titles)
  return (
    <>
      {/* 品牌色环境光:紫/粉两团定点光晕托住舞台,
          取代旧极光在浅色主题下发灰的随机漂移斑 */}
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-0 bg-[radial-gradient(560px_380px_at_50%_24%,rgba(168,85,247,.1),transparent_70%),radial-gradient(420px_300px_at_66%_62%,rgba(236,72,153,.07),transparent_72%)]"
      />
      {/* 开放式构图:不再用卡片框住,让品牌舞台直接呼吸 */}
      <TargetCursor className="relative z-[1] grid w-full max-w-[680px] place-items-center [padding:12px_20px_30px]">
        <div className="[animation:transcript-stage-enter_.5s_var(--ease-out)_both]">
          <WelcomeBrandStage />
        </div>
        <h2 className="text-[var(--accent-strong)] [animation:transcript-stage-enter_.55s_var(--ease-out)_both] [animation-delay:70ms]">
          <span
            className={`block [transition:opacity_.32s_ease,filter_.32s_ease,transform_.32s_ease] ${
              leaving ? 'translate-y-[4px] opacity-0 blur-[6px]' : 'opacity-100 blur-0'
            }`}
          >
            <BlurText key={title} text={title} />
          </span>
        </h2>
        {children}
      </TargetCursor>
    </>
  )
}
