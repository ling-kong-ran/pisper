// 当前输入也是有效凭据；加号只负责切换到下一项，不承担提交语义。
export function collectApiKeys(keys: string[], draft: string): string[] {
  return [...new Set([...keys, draft].map((key) => key.trim()).filter(Boolean))]
}
