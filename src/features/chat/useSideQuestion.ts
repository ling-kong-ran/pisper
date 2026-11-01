// 旁问（/btw）面板的状态：同一时间只保留一个问题，再问或关闭都会取消上一个请求；
// 切换会话或离开页面时丢弃。回答只存在这里，不写入会话，也不进草稿。
import { useCallback, useEffect, useRef, useState } from 'react'
import { chatApi } from './chat-api'

export type SideQuestionState = {
  question: string
  answer: string
  status: 'streaming' | 'done' | 'error'
  error: string
  toolCallIgnored: boolean
}

export function useSideQuestion(sessionId: string) {
  const [state, setState] = useState<SideQuestionState | null>(null)
  const controllerRef = useRef<AbortController | null>(null)

  const cancel = useCallback(() => {
    controllerRef.current?.abort()
    controllerRef.current = null
  }, [])

  const dismiss = useCallback(() => {
    cancel()
    setState(null)
  }, [cancel])

  useEffect(() => dismiss, [sessionId, dismiss])

  const ask = useCallback(
    (question: string) => {
      cancel()
      const controller = new AbortController()
      controllerRef.current = controller
      const owns = () => controllerRef.current === controller
      setState({ question, answer: '', status: 'streaming', error: '', toolCallIgnored: false })
      chatApi
        .askSideQuestion(sessionId, question, {
          signal: controller.signal,
          onDelta: (text) => {
            if (owns())
              setState((current) => current && { ...current, answer: current.answer + text })
          },
        })
        .then((result) => {
          if (!owns()) return
          controllerRef.current = null
          setState(
            (current) =>
              current && {
                ...current,
                // 以服务端终态为准，增量帧有遗漏时也能拿到完整回答。
                answer: result.text || current.answer,
                status: 'done',
                toolCallIgnored: result.toolCallIgnored,
              },
          )
        })
        .catch((error: unknown) => {
          if (!owns()) return
          controllerRef.current = null
          setState(
            (current) =>
              current && {
                ...current,
                status: 'error',
                error: error instanceof Error ? error.message : String(error),
              },
          )
        })
    },
    [cancel, sessionId],
  )

  return { state, ask, dismiss }
}
