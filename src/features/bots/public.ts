// @public Bots 的对外接口：路由页。侧栏分组在 public-sidebar.ts，两者都由调用方懒加载（不进首屏）。
export { BotsPage } from './BotsPage'
// @public Bot 群聊页（/bots/rooms/:roomId），随路由懒加载。
export { RoomPage } from './RoomPage'
