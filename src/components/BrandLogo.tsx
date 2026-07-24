// Vesper 品牌标：墨 V + 右上暖金四角星。
// V 用 currentColor 跟随文字色，星用 var(--star)，一处变色全局生效。
export function BrandLogo({ size = 22, className = '' }: { size?: number; className?: string }) {
  return (
    <svg
      className={`brand-logo-svg ${className}`}
      width={size}
      height={size}
      viewBox="0 0 48 48"
      role="img"
      aria-label="Vesper"
    >
      <path fill="currentColor" d="M3.5 13 L10.5 13 L21 29.5 L31.5 13 L38.5 13 L21 42 Z" />
      <path
        className="logo-star"
        fill="var(--star)"
        d="M41 1 Q41.9 5.1 46 6 Q41.9 6.9 41 11 Q40.1 6.9 36 6 Q40.1 5.1 41 1 Z"
      />
    </svg>
  )
}
