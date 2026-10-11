// 实际命令 Hook 和会话面板拥有状态；仅用内存目录替代产品会话加载。
import { useCallback, useMemo, useRef, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { SingleSessionPanel } from '@/features/chat/components/ChatDock'
import { ChatDockContext } from '@/features/chat/model/chat-dock-context'
import { useSessionCommands } from '@/features/chat/hooks/use-session-commands'
import { applySessionUpdate, DEFAULT_SESSION_STATE } from '@/lib/session/session-state'

const id = 'thinking-fixture'
const models = ['fixed', 'reasoner'].map((modelId) => ({
  key: `fixture/${modelId}`,
  provider: 'fixture',
  modelId,
  label: modelId,
  providerName: 'Fixture',
}))
const noop = () => {}
const asyncNoop = async () => {}
const notify = (message) => window.evidence.notices.push(message)
const requestText = () => Promise.resolve(null)
const requestConfirm = () => Promise.resolve(false)
window.evidence = { notices: [], completedReads: 0, refreshFinished: false }

export function Fixture() {
  const [availableModels, setAvailableModels] = useState(models)
  const [sessions, setSessions] = useState([
    { id, name: 'Thinking fixture', model: 'fixture/reasoner', thinkingLevel: 'medium' },
  ])
  const sessionsRef = useRef(sessions)
  const states = useRef({
    [id]: {
      ...DEFAULT_SESSION_STATE,
      loaded: true,
      model: 'fixture/reasoner',
      // 模拟旧能力残留；面板必须按当前模型自动刷新，不能被非空 status 挡住。
      thinkingLevel: 'medium',
      availableThinkingLevels: ['off'],
      thinkingStatus: 'ok',
    },
  })
  const listeners = useRef(new Map())
  const getSessionState = useCallback((sessionId) => states.current[sessionId], [])
  const subscribeSessionState = useCallback((sessionId, listener) => {
    const subscriptions = listeners.current.get(sessionId) || new Set()
    subscriptions.add(listener)
    listeners.current.set(sessionId, subscriptions)
    return () => subscriptions.delete(listener)
  }, [])
  const updateSessionState = useCallback((sessionId, update) => {
    const previous = states.current[sessionId]
    const next = applySessionUpdate(previous, update)
    if (next === previous) return
    states.current = { ...states.current, [sessionId]: next }
    for (const listener of listeners.current.get(sessionId) || []) listener()
  }, [])
  const updateSessions = useCallback((update) => {
    const next = typeof update === 'function' ? update(sessionsRef.current) : update
    sessionsRef.current = next
    setSessions(next)
    return next
  }, [])
  const commands = useSessionCommands({
    notify,
    requestText,
    requestConfirm,
    availableModels,
    sessionStatesRef: states,
    updateSessionState,
    updateSessions,
    replaceSessionStates: noop,
    setGlobalError: noop,
    syncLiveSession: asyncNoop,
  })
  const context = useMemo(
    () => ({
      sessions,
      activeId: id,
      defaultModel: 'fixture/reasoner',
      availableModels,
      subscribeSessionState,
      getSessionState,
      loadSessionMessages: asyncNoop,
      globalError: '',
      pendingAsset: null,
      contextTab: null,
      notify,
      ...commands,
    }),
    [availableModels, commands, getSessionState, sessions, subscribeSessionState],
  )
  window.controls = {
    inspect: () => states.current[id],
    removeModel: () =>
      setAvailableModels(models.filter((item) => item.key !== states.current[id].model)),
    patch: (update) => updateSessionState(id, update),
    switchModel: (model) => commands.switchSessionModel(id, model),
    refreshTwice: () => {
      window.evidence.refreshFinished = false
      void Promise.all([
        commands.loadSessionThinkingLevel(id, states.current[id].model),
        commands.loadSessionThinkingLevel(id, states.current[id].model),
      ]).then(() => {
        window.evidence.refreshFinished = true
      })
    },
  }
  return (
    <ChatDockContext.Provider value={context}>
      <SingleSessionPanel onCreateSession={asyncNoop} />
    </ChatDockContext.Provider>
  )
}

createRoot(document.getElementById('root')).render(<Fixture />)
