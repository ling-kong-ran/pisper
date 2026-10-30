// 会话页外壳（规格 4.1）：标题栏 48 → 泳道条 52（≤1279 宽或 ≤760 高时 36 单行）→ 会话页头 44 + 对话 + 检查器。
// 泳道条是看板折叠后的形态（看板领域），对话与检查器是聊天领域的单会话视图；两者都读同一个看板 store。
// URL 里的 sessionId 是选中依据：切换代理只换路由（replace，浏览器后退直接回看板），看板 store 不变；
// 路由组件在会话之间保持挂载，会话目录与转录状态不随切换重载。
// 手机外壳保持第 1 期的单会话视图（第 4 期重做）；手机 App 平板（≥960×640）走桌面布局、检查器为覆盖层。
// 本模块随会话路由按需加载，不进入入口包；聊天页由路由加载器单独懒加载后传入。
import { useCallback, useEffect, useMemo, useRef, useState, type ComponentType } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import {
  SessionLaneStrip,
  effectiveStripCurrent,
  type PendingStripSwitch,
} from '@/features/board/session-strip'
import type { ChatPage as ChatPageComponent } from '@/features/chat/ChatPage'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { useClientStore } from '@/stores/client-store'
import type { AppRouteContext } from './route-context'
import { NEW_SESSION_ROUTE_ID, PAGE_PATHS, sessionPath } from './routes'
import { SessionReturnBar } from './SessionReturnBar'

export const SESSION_COMPACT_QUERY = '(max-width: 1279px), (max-height: 760px)'
export const SESSION_TABLET_QUERY = '(min-width: 960px) and (min-height: 640px)'

// 媒体查询订阅放在随会话路由加载的模块里，不进入入口包。
function useMediaQuery(query: string) {
  const [matches, setMatches] = useState(() => window.matchMedia(query).matches)
  useEffect(() => {
    const media = window.matchMedia(query)
    const update = () => setMatches(media.matches)
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [query])
  return matches
}

type SessionShellProps = {
  context: AppRouteContext
  ChatPage: ComponentType<Parameters<typeof ChatPageComponent>[0]>
}

export function SessionShell({ context, ChatPage }: SessionShellProps) {
  const navigate = useNavigate()
  const { sessionId = '' } = useParams()
  const phone = usePhoneShell()
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  const tablet = useMediaQuery(SESSION_TABLET_QUERY) && mobileApp
  const compact = useMediaQuery(SESSION_COMPACT_QUERY)
  const pageLayout = !phone || tablet
  const currentRef = useRef(sessionId)
  currentRef.current = sessionId
  const openSession = useCallback(
    (id: string) => {
      if (!id || id === currentRef.current) return
      navigate(sessionPath(id), { replace: currentRef.current === NEW_SESSION_ROUTE_ID })
    },
    [navigate],
  )
  // 泳道条 / ⌥↑⌥↓ / ⌘J 切换代理：替换当前历史记录，浏览器后退仍然直接回到看板。
  // 路由在 transition 里提交，新会话还在加载时 sessionId 仍是旧值；记下已发出的目标，
  // 连按时以它为当前会话（effectiveStripCurrent），界面换过去之后清掉。
  const pendingSwitchRef = useRef<PendingStripSwitch | null>(null)
  useEffect(() => {
    if (pendingSwitchRef.current && pendingSwitchRef.current.from !== sessionId)
      pendingSwitchRef.current = null
  }, [sessionId])
  const resolveCurrentId = useCallback(
    () => effectiveStripCurrent(currentRef.current, pendingSwitchRef.current),
    [],
  )
  const switchSession = useCallback(
    (id: string) => {
      const from = currentRef.current
      if (!id || id === effectiveStripCurrent(from, pendingSwitchRef.current)) return
      pendingSwitchRef.current = { from, to: id }
      navigate(sessionPath(id), { replace: true })
    },
    [navigate],
  )
  const backToBoard = useCallback(() => navigate(PAGE_PATHS.board), [navigate])
  const pageChrome = useMemo(
    () => ({
      compact,
      overlayOnly: mobileApp,
      onOpenComponents: () => navigate(PAGE_PATHS.components),
      onArchived: () => navigate(PAGE_PATHS.board),
    }),
    [compact, mobileApp, navigate],
  )
  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 flex-col"
      data-session-shell={pageLayout ? 'desktop' : 'phone'}
    >
      {pageLayout && (
        <SessionLaneStrip
          currentId={sessionId}
          compact={compact}
          leading={<SessionReturnBar onBack={backToBoard} />}
          notify={context.notify}
          onOpenSession={switchSession}
          resolveCurrentId={resolveCurrentId}
        />
      )}
      <div className="flex min-h-0 min-w-0 flex-1">
        <ChatPage
          notify={context.notify}
          navigate={context.navigate}
          browserNotify={context.browserNotify}
          registerPrimaryAction={context.registerPrimaryAction}
          pendingAsset={context.pendingAsset}
          onAssetConsumed={context.onAssetConsumed}
          requestText={context.requestText}
          requestConfirm={context.requestConfirm}
          routeSessionId={sessionId}
          onRouteSessionChange={openSession}
          // 手机外壳：返回入口放进会话顶栏最左侧；桌面布局的返回键在泳道条上。
          routeLeading={pageLayout ? undefined : <SessionReturnBar onBack={backToBoard} />}
          pageChrome={pageLayout ? pageChrome : undefined}
        />
      </div>
    </div>
  )
}
