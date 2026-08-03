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
      <TargetCursor className="agent-welcome-content">
        <div className="welcome-visual">
          <BrandLogo size={54} className="welcome-logo" />
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
