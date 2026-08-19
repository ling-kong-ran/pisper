// 欢迎页动效:空会话时的品牌舞台与入场动效。
import type { ReactNode } from 'react'
import { BlurText } from '@/components/react-bits/BlurText'
import { TargetCursor } from '@/components/react-bits/TargetCursor'
import { WelcomeBrandStage } from './welcome-brand'

type WelcomeEffectsProps = {
  children: ReactNode
  title: string
}

export default function WelcomeEffects({ children, title }: WelcomeEffectsProps) {
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
        {/* 渐变必须落在每个词上:词元有 blur/filter 动画,
            祖先 bg-clip:text 会被后代 filter 打断导致文字隐形 */}
        <h2 className="[&_.rb-blur-text-word]:bg-[linear-gradient(100deg,#8B5CF6,#A855F7_48%,#EC4899)] [&_.rb-blur-text-word]:bg-clip-text [&_.rb-blur-text-word]:text-transparent [animation:transcript-stage-enter_.55s_var(--ease-out)_both] [animation-delay:70ms]">
          <BlurText text={title} />
        </h2>
        {children}
      </TargetCursor>
    </>
  )
}
