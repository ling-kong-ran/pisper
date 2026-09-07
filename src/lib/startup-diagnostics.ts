type StartupPhase =
  | 'react-app-mounted'
  | 'client-info-loaded'
  | 'capabilities-loaded'
  | 'config-loaded'
  | 'sessions-loaded'
  | 'chat-shell-ready'

const markedPhases = new Set<StartupPhase>()

export function markStartupPhase(phase: StartupPhase) {
  // 只在显式开启时记录固定阶段，不输出 URL、凭据或运行时负载。
  if (typeof window === 'undefined') return
  try {
    if (window.localStorage.getItem('pisper:startup-diagnostics') !== '1') return
  } catch {
    return
  }
  if (markedPhases.has(phase)) return
  markedPhases.add(phase)
  performance.mark(`pisper-${phase}`)
  console.debug('[startup]', phase, Math.round(performance.now()))
}
