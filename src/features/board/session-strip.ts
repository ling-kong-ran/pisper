// @public 会话页（应用壳的会话路由）用的泳道条；只在会话路由里懒加载，不进入入口包。
export {
  SessionLaneStrip,
  AGENT_PANEL_SHORTCUT,
  type SessionLaneStripProps,
} from './SessionLaneStrip'
export { effectiveStripCurrent, type PendingStripSwitch } from './strip-layout'
