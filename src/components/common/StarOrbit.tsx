// 星轨线条插画：空状态统一视觉（logo 星形元素衍生）。
export function StarOrbit({ size = 54, className = '' }: { size?: number; className?: string }) {
  return (
    <svg
      className={`star-orbit ${className}`}
      width={size}
      height={size}
      viewBox="0 0 64 64"
      fill="none"
      aria-hidden="true"
    >
      <circle
        cx="32"
        cy="32"
        r="23"
        stroke="var(--stroke)"
        strokeWidth="1.4"
        strokeDasharray="2.5 4.5"
        strokeLinecap="round"
      />
      <ellipse
        cx="32"
        cy="32"
        rx="28"
        ry="10.5"
        stroke="var(--stroke)"
        strokeWidth="1"
        transform="rotate(-18 32 32)"
        opacity=".65"
      />
      <circle cx="32" cy="32" r="3.2" fill="var(--control-muted)" />
      <circle cx="53" cy="27" r="1.7" fill="var(--star)" opacity=".8" />
      <circle cx="12" cy="45" r="1.3" fill="var(--text-muted)" opacity=".7" />
      <path
        d="M45 4 Q45.8 7.7 49.5 8.5 Q45.8 9.3 45 13 Q44.2 9.3 40.5 8.5 Q44.2 7.7 45 4 Z"
        fill="var(--star)"
      />
    </svg>
  )
}
