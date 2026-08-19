// 欢迎页动效:空会话时的品牌舞台与入场动效。
import type { ReactNode } from 'react'
import { Aurora } from '@/components/react-bits/Aurora'
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
      <Aurora />
      {/* 开放式构图:不再用卡片框住,让品牌舞台直接呼吸 */}
      <TargetCursor className="grid w-full max-w-[680px] place-items-center [padding:12px_20px_30px]">
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
