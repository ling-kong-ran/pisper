// @public 轻量契约：应用壳无须加载看板数据层即可派发；不导入页面，也没有导入副作用。
// “请求新任务”契约：⌘N、侧栏“新任务”、手机居中按钮与命令面板都只发这一个事件。
// 看板页挂载启动台后认领事件（preventDefault）并聚焦启动台；没有视图认领时，
// 应用壳打开派发浮层。事件只携带来源，不携带草稿，草稿由启动台/浮层自己持有。
import { useLayoutEffect, useRef } from 'react'

export const BOARD_NEW_TASK_EVENT = 'pisper:board-new-task'

export type BoardNewTaskSource = 'shortcut' | 'sidebar' | 'tabbar' | 'palette' | 'board'

export type BoardNewTaskRequest = { source: BoardNewTaskSource }

// 派发请求；返回 true 表示已有视图认领（看板启动台已聚焦），调用方不必再打开浮层。
export function requestBoardNewTask(source: BoardNewTaskSource): boolean {
  const event = new CustomEvent<BoardNewTaskRequest>(BOARD_NEW_TASK_EVENT, {
    detail: { source },
    cancelable: true,
  })
  return !window.dispatchEvent(event)
}

// 视图认领新任务请求：处理器返回 false 表示这次不认领（例如启动台暂不可用），交给浮层兜底。
export function useBoardNewTaskHandler(
  handler: (request: BoardNewTaskRequest) => boolean | void,
  enabled = true,
) {
  const handlerRef = useRef(handler)
  handlerRef.current = handler
  // 在提交阶段（绘制之前）认领：草稿一画出来就能接住 ⌘N，不会因被动 effect 晚一步而让外壳改开派发浮层。
  useLayoutEffect(() => {
    if (!enabled) return
    const listener = (event: Event) => {
      const detail = (event as CustomEvent<BoardNewTaskRequest>).detail
      if (!detail || handlerRef.current(detail) === false) return
      event.preventDefault()
    }
    window.addEventListener(BOARD_NEW_TASK_EVENT, listener)
    return () => window.removeEventListener(BOARD_NEW_TASK_EVENT, listener)
  }, [enabled])
}

// 全部代理页“/ 聚焦筛选”的目标：侧栏状态筛选的第一项“全部”（规格 2.3、2.8）。
// 应用壳的 focusSearch 在全部代理页聚焦它；侧栏收成窄轨时先临时展开。
export const BOARD_FILTER_ID = 'board-filter'
