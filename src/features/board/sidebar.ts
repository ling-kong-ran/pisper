// @public 应用壳侧栏用的会话分组、窄轨计数与命令处理；只由外壳懒加载，不进入入口包。
// 分组、计数与命令都读看板 store（单一数据所有者），文案在看板命名空间里，由外壳随块一起注册。
export {
  SidebarCommands,
  SidebarRailCounts,
  SidebarSessions,
  type SidebarCommandsProps,
  type SidebarRailCountsProps,
  type SidebarSessionsProps,
} from './SidebarSessions'
