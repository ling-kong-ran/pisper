// 欢迎页品牌舞台:Logo + 分支轨道环,呼应「开分支」的产品叙事。
// 轻量 SVG,主包直出,加载态与完整动效版共用同一构图,避免布局跳动。
import { useId } from 'react'
import { BrandLogo } from '@/components/BrandLogo'

export function WelcomeBrandStage() {
  // 分屏下可能同时存在多个舞台,渐变 id 必须实例唯一
  const gradientId = useId()
  return (
    <div className="relative grid h-[124px] w-[124px] place-items-center" aria-hidden="true">
      {/* 极光底晕,缓慢呼吸 */}
      <div className="absolute inset-[4px] rounded-full bg-[radial-gradient(circle,rgba(168,85,247,.17),transparent_68%)] [animation:agent-aura-think_3.6s_ease-in-out_infinite]" />
      {/* 分支轨道环:环上三个节点,像从主干分出的三条枝 */}
      <svg
        className="absolute inset-0 h-full w-full [animation:galaxy-orbit-drift_22s_linear_infinite]"
        viewBox="0 0 124 124"
        fill="none"
      >
        <defs>
          <linearGradient
            id={gradientId}
            x1="0"
            y1="0"
            x2="124"
            y2="124"
            gradientUnits="userSpaceOnUse"
          >
            <stop offset="0" stopColor="#8B5CF6" />
            <stop offset="0.55" stopColor="#A855F7" />
            <stop offset="1" stopColor="#EC4899" />
          </linearGradient>
        </defs>
        <circle
          cx="62"
          cy="62"
          r="52"
          stroke={`url(#${gradientId})`}
          strokeWidth="1.2"
          strokeDasharray="2.5 7"
          opacity="0.7"
        />
        <circle cx="62" cy="10" r="3.2" fill="#A855F7" />
        <circle cx="107" cy="88" r="2.6" fill="#EC4899" />
        <circle cx="17" cy="88" r="2.2" fill="#1783ff" />
      </svg>
      <div className="grid h-[68px] w-[68px] place-items-center rounded-[22px] border border-[color-mix(in_srgb,#A855F7_24%,var(--stroke))] bg-[var(--solid)] shadow-[0_18px_44px_-24px_rgba(168,85,247,.45)]">
        <BrandLogo size={40} />
      </div>
    </div>
  )
}
