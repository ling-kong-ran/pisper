// 欢迎页动效：空会话时的品牌展示与打字机/星轨装饰。
import type { ReactNode } from 'react'
import { BrandLogo } from '@/components/BrandLogo'
import { AsciiText } from '@/components/react-bits/AsciiText'
import { Aurora } from '@/components/react-bits/Aurora'
import { BlurText } from '@/components/react-bits/BlurText'
import { TargetCursor } from '@/components/react-bits/TargetCursor'

type WelcomeEffectsProps = {
  children: ReactNode
  title: string
}

export default function WelcomeEffects({ children, title }: WelcomeEffectsProps) {
  return (
    <>
      <Aurora />
      <TargetCursor className="agent-welcome-content [:root[data-theme='light']_&]:[border:1px_solid_color-mix(in_srgb,_var(--brand-blue)_13%,_var(--stroke))] [:root[data-theme='light']_&]:rounded-[32px] [:root[data-theme='light']_&]:bg-[radial-gradient(circle_at_16%_8%,_color-mix(in_srgb,_var(--brand-blue)_10%,_transparent),_transparent_38%),_radial-gradient(circle_at_88%_92%,_rgba(139,_92,_246,_.08),_transparent_42%),_rgba(255,255,255,.82)] [:root[data-theme='light']_&]:shadow-[0_28px_72px_-48px_rgba(30,64,175,.3),_0_8px_26px_-22px_rgba(15,23,42,.22)] [:root[data-theme='light']_&]:[backdrop-filter:blur(12px)] grid w-full max-w-[640px] place-items-center [padding:28px]">
        <div className="welcome-visual [:root[data-theme='light']_&]:relative [:root[data-theme='light']_&::before]:absolute [:root[data-theme='light']_&::before]:top-[-8px] [:root[data-theme='light']_&::before]:w-[78px] [:root[data-theme='light']_&::before]:h-[78px] [:root[data-theme='light']_&::before]:[border:1px_solid_rgba(23,131,255,.12)] [:root[data-theme='light']_&::before]:rounded-[28px] [:root[data-theme='light']_&::before]:bg-[linear-gradient(145deg,_rgba(255,255,255,.94),_rgba(239,246,255,.76))] [:root[data-theme='light']_&::before]:shadow-[0_18px_42px_-26px_rgba(23,131,255,.48)] [:root[data-theme='light']_&::before]:[content:''] [:root[data-theme='light']_&::before]:[transform:rotate(8deg)] grid min-h-[144px] place-items-center">
          <BrandLogo
            size={54}
            className="welcome-logo [.agent-welcome_&]:relative [.agent-welcome_&]:z-[1] [.agent-welcome_&]:text-[var(--text)] [:root[data-theme='light']_.agent-welcome_&]:[filter:drop-shadow(0_8px_14px_rgba(23,131,255,.12))]"
          />
          <AsciiText text="PISPER" />
        </div>
        <h2>
          <BlurText text={title} />
        </h2>
        {children}
      </TargetCursor>
    </>
  )
}
