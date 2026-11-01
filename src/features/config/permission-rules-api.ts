// 持久权限规则的领域接口：设置页管理规则，审批卡片“以后都允许”也经这里添加。
import { apiJson } from '@/lib/api'

export type PermissionRule = {
  id: string
  effect: 'allow' | 'deny'
  tool: 'bash' | 'files'
  pattern: string
  // 空字符串表示全局；否则是项目目录。
  cwd: string
  createdAt: string
}

export type PermissionRuleDraft = Pick<PermissionRule, 'effect' | 'tool' | 'pattern'> & {
  cwd?: string
}

type PermissionRuleList = { rules: PermissionRule[] }

export const permissionRulesApi = {
  list() {
    return apiJson<PermissionRuleList>('/api/permission-rules')
  },
  add(draft: PermissionRuleDraft) {
    return apiJson<PermissionRuleList>('/api/permission-rules', {
      method: 'POST',
      body: JSON.stringify(draft),
    })
  },
  remove(id: string) {
    return apiJson<PermissionRuleList>('/api/permission-rules/' + encodeURIComponent(id), {
      method: 'DELETE',
    })
  },
}

// 与 Runtime 的允许规则限制一致：含管道、重定向、变量等控制符的命令不能作为允许规则。
const SHELL_CONTROL_PATTERN = /[;&|`$<>\n\r\\(){}]/

// 审批卡片上“以后都允许”建议的命令前缀：取命令名，第二个词像子命令（npm test、git status）时一并带上；
// 带控制符的命令不给建议，让用户逐条确认。
export function suggestCommandRulePrefix(command: string) {
  const normalized = command.trim().replace(/[ \t]+/g, ' ')
  if (!normalized || SHELL_CONTROL_PATTERN.test(normalized)) return ''
  const [first, second] = normalized.split(' ')
  if (!first) return ''
  return second && /^[a-z][\w:-]*$/i.test(second) ? `${first} ${second}` : first
}
