// Pisper 品牌标：墨色方形键帽 + 反白四芒星与一颗卫星点。
// 单色是为了让品牌只在功能栏顶部、智能体头像等少数位置出现，不与唯一强调色争夺注意力；
// 颜色全部取自 --ink / --on-ink，暗色主题自动翻转为浅色键帽 + 深色星。
export function BrandLogo({ size = 22, className = '' }: { size?: number; className?: string }) {
  return (
    <svg
      className={`block flex-none ${className}`}
      width={size}
      height={size}
      viewBox="0 0 34 34"
      role="img"
      aria-label="Pisper"
    >
      <rect width="34" height="34" rx="9" fill="var(--ink)" />
      <g fill="var(--on-ink)" transform="translate(7 7) scale(.8333)">
        <path d="M12 3.2c.55 4.6 3.6 7.65 8.2 8.2-4.6.55-7.65 3.6-8.2 8.2-.55-4.6-3.6-7.65-8.2-8.2 4.6-.55 7.65-3.6 8.2-8.2z" />
        <circle cx="19.4" cy="5.1" r="1.25" />
      </g>
    </svg>
  )
}
