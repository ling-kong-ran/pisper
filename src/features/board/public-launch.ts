// @public 给已有会话发任务消息（组件工作室的“生成”“让代理修复”）：流由看板 store 持有，会话进入“运行中”。
// 只转出这一个函数与类型；不导入页面，也没有导入副作用。
export {
  sendBoardTaskMessage,
  BoardLaunchError,
  type BoardTaskMessage,
  type BoardTaskMessageResult,
} from './board-launch'
