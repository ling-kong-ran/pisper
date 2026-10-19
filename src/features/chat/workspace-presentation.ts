// 局部宽度只改变显示方式，不修改任务、标签或桌面分屏的持久化结构。
export function resolveWorkspacePresentation({
  width,
  mobile,
  contentOpen,
}: {
  width: number
  mobile: boolean
  contentOpen: boolean
}): 'conversation' | 'split' | 'content' {
  if (!contentOpen) return 'conversation'
  return mobile || width < 660 ? 'content' : 'split'
}
