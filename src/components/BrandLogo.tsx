// Pisper 品牌标：终端 P（竖杆 + ">" 字碗 + 光标），紫罗兰极光渐变。
export function BrandLogo({ size = 22, className = '' }: { size?: number; className?: string }) {
  return (
    <svg
      className={`block flex-none text-[var(--text)] ${className}`}
      width={size}
      height={size}
      viewBox="0 0 48 48"
      role="img"
      aria-label="Pisper"
    >
      <defs>
        <linearGradient
          id="pisper-brand-gradient"
          x1="9"
          y1="6"
          x2="40"
          y2="44"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0" stopColor="#8B5CF6" />
          <stop offset="0.55" stopColor="#A855F7" />
          <stop offset="1" stopColor="#EC4899" />
        </linearGradient>
      </defs>
      <g
        fill="none"
        stroke="url(#pisper-brand-gradient)"
        strokeWidth={4.5}
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M13,7 V42" />
        <path d="M13,7 L37,20.5 L13,34" />
      </g>
      <rect x={23} y={38} width={15} height={4} rx={2} fill="url(#pisper-brand-gradient)" />
    </svg>
  )
}
