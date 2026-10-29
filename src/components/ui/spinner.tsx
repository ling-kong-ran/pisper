import { cn } from '@/lib/utils'

// 品牌运行指示「轨道」：静止的细圆环 + 绕行的卫星点。颜色跟随 currentColor（默认强调色文字），
// 圆环旋转对称，所以直接旋转根 svg 与只转卫星观感一致；旋转放在根节点上，是为了让
// `svg.animate-spin` 这类选择器（UI 冒烟据此判断运行指示是否出现/消失）如实命中。
// 动画只改 transform；减少动态时由全局规则停在初始帧，仍保留完整图形。
function OrbitSpinner({ className, ...props }: React.ComponentProps<'svg'>) {
  return (
    <svg
      data-slot="orbit-spinner"
      aria-hidden="true"
      viewBox="0 0 16 16"
      fill="none"
      className={cn('size-4 shrink-0 origin-center animate-spin text-accent-fg', className)}
      {...props}
    >
      <circle cx="8" cy="8" r="5.25" stroke="currentColor" strokeOpacity=".28" strokeWidth="1.5" />
      <circle cx="8" cy="2.75" r="2" fill="currentColor" />
    </svg>
  )
}

function Spinner({ className, ...props }: React.ComponentProps<'svg'>) {
  return (
    <OrbitSpinner
      data-slot="spinner"
      role="status"
      aria-label="Loading"
      aria-hidden={undefined}
      className={className}
      {...props}
    />
  )
}

export { OrbitSpinner, Spinner }
