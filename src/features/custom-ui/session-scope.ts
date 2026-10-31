// 会话面板实例的范围（计划 §3.3）：{type:'all'} 在所有会话里显示，{type:'project', cwd} 只在同一项目的会话里显示。
// 项目按看板同一规则比较（boardWorkspaceKey：统一分隔符、去掉尾斜杠、Windows 盘符不分大小写）。
// 检查器标签（经 placements.ts）与“+”选择器（hosts/*）共用这一条规则，单独成文件以免 hosts 反向导入 placements。
import { boardWorkspaceKey } from '@shared/session-board-status.mjs'
import type { ComponentInstanceScope } from './custom-ui-api'

export function sessionScopeMatches(scope: ComponentInstanceScope | null, cwd: string): boolean {
  if (!scope || scope.type === 'all') return true
  const key = boardWorkspaceKey(cwd)
  return key !== '' && boardWorkspaceKey(scope.cwd) === key
}
