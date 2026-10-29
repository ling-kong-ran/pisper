// 会话页（路由模式）注入到会话顶栏最左侧的返回入口。由应用壳经 ChatPage 传入，
// 聊天领域只负责摆放，不关心它是什么；非路由模式（旧 Dock、手机多会话）为空。
import { createContext, type ReactNode } from 'react'

export const SessionRouteLeading = createContext<ReactNode>(null)
