// 真实 Dock、面板生命周期和会话事件；模型内容与目录仅使用固定内存数据。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { ChatDockView } from '@/features/chat/components/ChatDockView'
import { SingleSessionPanel } from '@/features/chat/components/ChatDock'
import { ChatDockContext } from '@/features/chat/model/chat-dock-context'
import { useChatDock } from '@/features/chat/hooks/use-chat-dock'
import { announceSessionsUpdated, requestSessionSelection } from '@/features/chat/model/events'

const catalog = [
  { id: 'alpha', name: 'Alpha', cwd: '/fixture' },
  { id: 'beta', name: 'Beta', cwd: '/fixture' },
  { id: 'gamma', name: 'Gamma', cwd: '/fixture' },
]
const noop = () => {}
const subscribe = () => noop
const asyncNoop = () => Promise.resolve()
window.evidence = {
  reads: 0,
  writes: 0,
  writeAttempts: 0,
  loads: [],
  releases: [],
  notices: [],
  stopped: [],
}

function DockFixture() {
  const deleted = new URLSearchParams(location.search).get('deleted')
  const [sessions, setSessions] = useState(() => catalog.filter(({ id }) => id !== deleted))
  const sessionsRef = useRef(sessions)
  sessionsRef.current = sessions
  const [activeId, setActiveId] = useState('alpha')
  const [phone, setPhone] = useState(() => matchMedia('(max-width: 650px)').matches)
  const [disposition, setDisposition] = useState('open')
  const states = useRef(
    Object.fromEntries(
      catalog.map(({ id }) => [id, { loaded: true, messages: [], streaming: false }]),
    ),
  )
  const localStreamSessionsRef = useRef(new Set())
  const loadSessionMessages = useCallback((id) => {
    window.evidence.loads.push(id)
    return Promise.resolve()
  }, [])
  const releaseSessionState = useCallback((id, options) => {
    window.evidence.releases.push({ id, ...options })
    if (options.panelOpen || options.localStreamOwned) return false
    if (localStreamSessionsRef.current.has(id)) window.evidence.stopped.push(id)
    return true
  }, [])
  const getSessionState = useCallback((id) => states.current[id], [])
  const notify = useCallback((message) => window.evidence.notices.push(message), [])
  const dock = useChatDock({
    sessions,
    sessionsRef,
    sessionStates: states.current,
    activeId,
    setActiveId,
    loading: false,
    localStreamSessionsRef,
    loadSessionMessages,
    releaseSessionState,
    singleSessionLayout: phone,
    notify,
  })
  useEffect(() => {
    const media = matchMedia('(max-width: 650px)')
    const update = () => setPhone(media.matches)
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])
  const createSession = useCallback(() => {
    requestSessionSelection('gamma')
    return Promise.resolve('gamma')
  }, [])
  const context = useMemo(
    () => ({
      sessions,
      activeId,
      compactDock: dock.compactDock,
      subscribeSessionState: subscribe,
      getSessionState,
      loadSessionMessages,
      loadSessionThinkingLevel: asyncNoop,
      defaultModel: 'fixture',
      availableModels: [],
      globalError: '',
      pendingAsset: null,
      contextTab: null,
      splitDockPanel: dock.splitDockPanel,
      closeDockPanel: dock.closeDockPanel,
      notify,
    }),
    [
      activeId,
      dock.closeDockPanel,
      dock.compactDock,
      dock.splitDockPanel,
      getSessionState,
      loadSessionMessages,
      notify,
      sessions,
    ],
  )
  const deleteSession = (id) => {
    setSessions((current) => current.filter((session) => session.id !== id))
    announceSessionsUpdated({ deletedIds: [id] })
  }
  window.controls = {
    startStream: (id) => {
      localStreamSessionsRef.current.add(id)
    },
    inspect: () => ({
      activeId,
      phone,
      localStreams: [...localStreamSessionsRef.current],
    }),
  }
  return (
    <ChatDockContext.Provider value={context}>
      <nav
        aria-label="Fixture session controls"
        style={{ display: 'flex', flexWrap: 'wrap', gap: 8, padding: 8 }}
      >
        <select
          aria-label="Open position"
          value={disposition}
          onChange={(event) => setDisposition(event.target.value)}
        >
          {['open', 'left', 'right', 'above', 'below'].map((value) => (
            <option key={value}>{value}</option>
          ))}
        </select>
        {sessions.map((session) => (
          <button key={session.id} onClick={() => requestSessionSelection(session.id, disposition)}>
            Open {session.name}
          </button>
        ))}
        <button onClick={() => deleteSession('beta')}>Delete Beta</button>
        <button onClick={() => deleteSession('alpha')}>Delete Alpha</button>
        <output data-active-id={activeId}>Active: {activeId}</output>
      </nav>
      <main style={{ flex: 1, minHeight: 0, minWidth: 0 }} data-mode={phone ? 'phone' : 'desktop'}>
        {phone ? (
          <SingleSessionPanel onCreateSession={createSession} />
        ) : (
          <ChatDockView
            compactDock={dock.compactDock}
            onDockReady={dock.onDockReady}
            getTabContextMenuItems={dock.getTabContextMenuItems}
            createSession={createSession}
          />
        )}
      </main>
    </ChatDockContext.Provider>
  )
}

function Fixture() {
  const [mounted, setMounted] = useState(true)
  window.setFixtureMounted = setMounted
  return mounted ? <DockFixture /> : <div>Unmounted</div>
}
createRoot(document.getElementById('root')).render(<Fixture />)
