// 会话页运行中输入的方式（规格 2.8 / 4.3）：↵ 排队（/input followUp），⌥↵ 插话（/input steer）。
// 纯函数，供输入框与测试共用。
// 会话页“⌥↵ 插话”：在当前发送绑定上加 Alt（发送绑定本身带 Alt 时没有插话键）。
export function steerBindingFor(binding: string | null | undefined) {
  if (!binding) return null
  const parts = binding.split('+')
  const code = parts.pop()
  if (!code || parts.includes('Alt')) return null
  const primary = parts.filter((part) => part !== 'Shift')
  return [...primary, 'Alt', ...(parts.includes('Shift') ? ['Shift'] : []), code].join('+')
}
