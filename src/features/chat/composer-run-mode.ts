// 输入框的执行方式（Plan / Goal / Team）与 ⇧Tab 切换顺序；盾牌芯片与输入框键盘处理共用。
export type ComposerExecutionMode = 'plan' | 'goal' | 'team'

// ⇧Tab 的切换顺序：Plan → Goal → Team → Plan；不可用的方式跳过，只有 Plan 可用时不切换（返回 null）。
export function nextComposerRunMode(
  mode: ComposerExecutionMode,
  options: { goalsAvailable: boolean; teamAvailable: boolean },
): ComposerExecutionMode | null {
  const order: ComposerExecutionMode[] = ['plan']
  if (options.goalsAvailable) order.push('goal')
  if (options.goalsAvailable && options.teamAvailable) order.push('team')
  if (order.length < 2) return null
  const index = order.indexOf(mode)
  return order[(index + 1) % order.length] ?? 'plan'
}
