import { useId } from 'react'

// Pisper 品牌标：深空圆角底 + 终端 P（竖杆 + ">" 字碗 + 光标），极光渐变。
// 与官网 docs/brand/pisper-logo.svg、public/favicon.svg 和应用图标同一图形，界面里不再另用一套标志。
// 深色底自带对比度，亮暗主题下都不用翻色。
export function BrandLogo({ size = 22, className = '' }: { size?: number; className?: string }) {
  // 同页会同时出现多个标志（侧栏、每条助手消息），渐变 id 必须各自唯一，
  // 否则先渲染的那个被隐藏时，其余实例会丢失渐变。
  const gradient = `pisper-brand-${useId().replace(/:/g, '')}`
  return (
    <svg
      className={`block flex-none ${className}`}
      width={size}
      height={size}
      viewBox="0 0 48 48"
      role="img"
      aria-label="Pisper"
    >
      <defs>
        <linearGradient id={gradient} x1="9" y1="6" x2="40" y2="44" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#8B5CF6" />
          <stop offset="0.55" stopColor="#A855F7" />
          <stop offset="1" stopColor="#EC4899" />
        </linearGradient>
      </defs>
      <rect width="48" height="48" rx="11" fill="#0B0F1E" />
      <g fill="#FFFFFF">
        <circle cx="8" cy="9" r="0.5" opacity="0.5" />
        <circle cx="40" cy="8" r="0.6" opacity="0.5" />
        <circle cx="44" cy="20" r="0.5" opacity="0.4" />
        <circle cx="6" cy="28" r="0.5" opacity="0.4" />
        <circle cx="9" cy="41" r="0.6" opacity="0.45" />
        <circle cx="41" cy="38" r="0.5" opacity="0.4" />
      </g>
      <g
        fill="none"
        stroke={`url(#${gradient})`}
        strokeWidth="4.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M13,7 V42" />
        <path d="M13,7 L37,20.5 L13,34" />
      </g>
      <rect x="23" y="38" width="15" height="4" rx="2" fill={`url(#${gradient})`} />
    </svg>
  )
}
