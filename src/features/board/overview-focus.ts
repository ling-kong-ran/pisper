// 侧栏命令（⌥A）交给全部代理页的“聚焦这一行”请求。页面自己决定怎么到达那一行：虚拟列表里先滚过去，
// 被筛选藏起来时先清筛选。侧栏只知道会话 id，不去 DOM 里找行（虚拟列表里不在视口的行根本没挂载）。
export const OVERVIEW_FOCUS_EVENT = 'pisper:overview-focus'

export type OverviewFocusDetail = { sessionId: string }

export function requestOverviewFocus(sessionId: string) {
  window.dispatchEvent(
    new CustomEvent<OverviewFocusDetail>(OVERVIEW_FOCUS_EVENT, { detail: { sessionId } }),
  )
}
