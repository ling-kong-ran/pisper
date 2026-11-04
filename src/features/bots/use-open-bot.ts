// 打开 Bot = 打开它的长期会话（普通会话页：实时回复、插话、附件、审批都是现成的）。
// 每次都先问 Runtime 要会话 id：旧数据没有会话、或会话文件已经不在时由它补建，不会跳到“找不到会话”。
import { useCallback } from 'react'
import { useNavigate } from 'react-router-dom'
import { sessionPath } from '@/app/routes'
import { openBotSession, type Bot } from './bots-api'
import { useBotsStore } from './bots-store'

export function useOpenBot(onError: (message: string, botId: string) => void) {
  const navigate = useNavigate()
  const load = useBotsStore((state) => state.load)
  return useCallback(
    async (bot: Pick<Bot, 'id' | 'threadSessionId'>) => {
      try {
        const sessionId = await openBotSession(bot.id)
        if (sessionId !== bot.threadSessionId) void load()
        navigate(sessionPath(sessionId))
      } catch (caught) {
        onError(caught instanceof Error ? caught.message : String(caught), bot.id)
      }
    },
    [load, navigate, onError],
  )
}
