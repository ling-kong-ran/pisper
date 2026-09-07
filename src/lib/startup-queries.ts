import { QueryClient } from '@tanstack/react-query'
import { apiJson } from './api'
import { markStartupPhase } from './startup-diagnostics'

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: { refetchOnWindowFocus: false, retry: 1, staleTime: 15_000 },
    mutations: { retry: 0 },
  },
})

const paths = {
  config: '/api/config',
  sessions: '/api/sessions',
  'notification-settings': '/api/settings/notifications',
  plugins: '/api/plugins',
} as const

type StartupQuery = keyof typeof paths
const versions = new Map<StartupQuery, number>()

export function startupQueryOptions<T>(key: StartupQuery) {
  return {
    queryKey: [key],
    // 不消费观察者的取消信号，让 StrictMode 重挂与命令式读取共享在途请求。
    queryFn: async () => {
      for (;;) {
        const version = versions.get(key)
        try {
          const data = await apiJson<T>(paths[key])
          // 连续事件只推进版本；旧请求完成后合并补读，不取消重发形成请求风暴。
          if (version !== versions.get(key)) continue
          if (key === 'sessions') markStartupPhase('sessions-loaded')
          return data
        } catch (error) {
          if (version !== versions.get(key)) continue
          throw error
        }
      }
    },
    staleTime: 15_000,
  }
}

export function fetchStartupQuery<T>(key: StartupQuery, refresh = false): Promise<T> {
  return queryClient.fetchQuery({
    ...startupQueryOptions<T>(key),
    ...(refresh ? { staleTime: 0 } : {}),
  })
}

export function invalidateStartupQuery(key: StartupQuery) {
  versions.set(key, (versions.get(key) ?? 0) + 1)
  return queryClient.invalidateQueries({ queryKey: [key], exact: true }, { cancelRefetch: false })
}

export function installStartupQueryEvents(target: EventTarget) {
  const refreshSessions = () => {
    void invalidateStartupQuery('sessions')
  }
  target.addEventListener('pisper:sessions-updated', refreshSessions)
  return () => target.removeEventListener('pisper:sessions-updated', refreshSessions)
}
