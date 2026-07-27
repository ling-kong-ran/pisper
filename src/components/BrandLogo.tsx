// Pisper 品牌标：终端 P（竖杆 + ">" 字碗 + 光标），跟随 currentColor，一处变色全局生效。
export function BrandLogo({ size = 22, className = '' }: { size?: number; className?: string }) {
  return (
    <svg
      className={`brand-logo-svg ${className}`}
      width={size}
      height={size}
      viewBox="0 0 48 48"
      role="img"
      aria-label="Pisper"
    >
      <g
        fill="none"
        stroke="currentColor"
        strokeWidth={4.5}
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M13,7 V42" />
        <path d="M13,7 L37,20.5 L13,34" />
      </g>
      <rect x={23} y={38} width={15} height={4} rx={2} fill="currentColor" />
    </svg>
  )
}
