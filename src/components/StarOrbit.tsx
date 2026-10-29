// 星轨插画：空状态的品牌时刻——虚线轨道、内环、低调四芒星与一颗强调色行星。
// 只用描边/文字 token 着色，强调色只落在行星上；size 为高度（沿用旧方形插画的占位），宽度按 4:3 画幅换算。
export function StarOrbit({ size = 48, className = '' }: { size?: number; className?: string }) {
  return (
    <svg
      className={`star-orbit block flex-none ${className}`}
      width={Math.round((size * 4) / 3)}
      height={size}
      viewBox="0 0 64 48"
      fill="none"
      aria-hidden="true"
    >
      <ellipse
        cx="32"
        cy="24"
        rx="28"
        ry="11"
        transform="rotate(-14 32 24)"
        stroke="var(--stroke-strong)"
        strokeDasharray="2 3"
      />
      <ellipse
        cx="32"
        cy="24"
        rx="16"
        ry="6"
        transform="rotate(-14 32 24)"
        stroke="var(--stroke)"
      />
      <path
        d="M32 15.5c.55 4.3 3.2 6.95 7.5 7.5-4.3.55-6.95 3.2-7.5 7.5-.55-4.3-3.2-6.95-7.5-7.5 4.3-.55 6.95-3.2 7.5-7.5z"
        fill="var(--text-muted)"
      />
      <circle cx="56.6" cy="14" r="2.6" fill="var(--star)" />
      <circle cx="8" cy="8" r="1" fill="var(--text-faint)" />
      <circle cx="13" cy="41" r=".9" fill="var(--text-faint)" />
      <circle cx="55" cy="40" r="1.1" fill="var(--text-faint)" />
    </svg>
  )
}
