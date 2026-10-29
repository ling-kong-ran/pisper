// 视图挂载期间持有看板同步：多个视图共用同一个轮询者，最后一个卸载后统一清理。
import { useEffect } from 'react'
import { retainBoardSync } from './board-store'

export function useBoardSync() {
  useEffect(() => retainBoardSync(), [])
}
