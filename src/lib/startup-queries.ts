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

// 会话目录的唯一权威缓存：看板、侧栏、聊天目录垫片和应用壳都读写这一份。
// 第 1 期看板视图与完整目录是同一个 /api/sessions，因此直接使用看板键，避免同一数据出现两份缓存。
export const SESSIONS_QUERY_KEY = ['sessions', 'board'] as const

function startupQueryKey(key: StartupQuery): readonly string[] {
  return key === 'sessions' ? SESSIONS_QUERY_KEY : [key]
}

export function startupQueryOptions<T>(key: StartupQuery) {
  return {
    queryKey: startupQueryKey(key),
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
  return queryClient.invalidateQueries(
    { queryKey: startupQueryKey(key), exact: true },
    { cancelRefetch: false },
  )
}

export function installStartupQueryEvents(target: EventTarget) {
  const refreshSessions = () => {
    void invalidateStartupQuery('sessions')
  }
  target.addEventListener('pisper:sessions-updated', refreshSessions)
  return () => target.removeEventListener('pisper:sessions-updated', refreshSessions)
}
