import type { ReactNode } from 'react'
import { WelcomeBrandStage } from './welcome-brand'

export default function WelcomeEffects({
  children,
  titles,
}: {
  children: ReactNode
  titles: string[]
}) {
  return (
    <div className="relative grid w-full max-w-[680px] justify-items-center gap-5 px-4 py-6">
      <WelcomeBrandStage />
      <h2 className="text-[clamp(22px,2.4vw,30px)] font-medium tracking-tight text-foreground">
        {titles[0]}
      </h2>
      {children}
    </div>
  )
}
