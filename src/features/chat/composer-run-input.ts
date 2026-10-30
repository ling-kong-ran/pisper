// 会话页运行中输入的方式（规格 2.8 / 4.3）：↵ 排队（/input followUp），⌥↵ 插话（/input steer）。
// 第 4 期起插话键是 shortcut-store 里的独立动作 steerMessage（默认 Alt+Enter，可改绑），
// 输入框不再调用这里的派生规则；函数与其单元测试暂留，第 6 期清理时一并删除。
// 旧规则：在当前发送绑定上加 Alt（发送绑定本身带 Alt 时没有插话键）。
export function steerBindingFor(binding: string | null | undefined) {
  if (!binding) return null
  const parts = binding.split('+')
  const code = parts.pop()
  if (!code || parts.includes('Alt')) return null
  const primary = parts.filter((part) => part !== 'Shift')
  return [...primary, 'Alt', ...(parts.includes('Shift') ? ['Shift'] : []), code].join('+')
}
