// 会话页外壳（规格 v2 第 4.1 节）：会话页头 + 对话 + 检查器；并行的代理在应用侧栏里（规格 2.1），
// 会话页不再有泳道条与 ⌘J 代理面板。对话与检查器是聊天领域的单会话视图。
// URL 里的 sessionId 是选中依据：侧栏切换会话只换路由（replace，浏览器后退回到来处），看板 store 不变；
// 路由组件在会话之间保持挂载，会话目录与转录状态不随切换重载。
// 进入真实会话时记下“上次停留的会话”，主页据此落地；归档当前会话后回到主页（此时显示新任务草稿）。
// replace 切换时保留历史状态，从主页落地的会话切换代理后仍是根（手机原生返回交还平台）。
// 手机外壳（规格 v2 第 7.1、7.2 节）：顶栏 56（左侧 ☰ 带需要你计数，点开抽屉）+ 对话，检查器改为底部表单；
// 第 4 期的代理条与“看板 N”入口退役，并行的代理在抽屉里。手机 App 平板（≥960×640）走桌面布局、检查器为覆盖层。
// 检查器的组件标签（“会话面板”放置，规格 4.4）由这里按放置实例构造（SessionPageChrome.componentTabs），
// 聊天领域只认类型契约；组件宿主经 placements.ts 的懒加载边界进入，没有实例时不加载。
// 本模块随会话路由按需加载，不进入入口包；聊天页由路由加载器单独懒加载后传入。
import {
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ComponentType,
} from 'react'
import { useLocation, useNavigate, useParams } from 'react-router-dom'
import { useSessionListQuery } from '@/features/board/public'
import type { ChatPage as ChatPageComponent } from '@/features/chat/ChatPage'
import type { SessionComponentTabs } from '@/features/chat/session-workspace-context'
import {
  LazySessionComponentAddMenu,
  LazySessionPanelComponent,
  sessionPanelTabs,
  useComponentInstances,
} from '@/features/custom-ui/placements'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { useClientStore } from '@/stores/client-store'
import type { AppRouteContext } from './route-context'
import { useI18n } from './use-i18n'
import { PhoneMenuButton } from './PhoneMenuButton'
import { HOME_PATH, NEW_SESSION_ROUTE_ID, PAGE_PATHS, sessionPath } from './routes'
import { forgetLastSession, rememberLastSession } from './session-home'

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
  const location = useLocation()
  const { sessionId = '' } = useParams()
  const phone = usePhoneShell()
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  const tablet = useMediaQuery(SESSION_TABLET_QUERY) && mobileApp
  const compact = useMediaQuery(SESSION_COMPACT_QUERY)
  const pageLayout = !phone || tablet
  const currentRef = useRef(sessionId)
  currentRef.current = sessionId
  // 当前历史条目的状态（主页落地标记）：replace 时原样带上，push 新条目时不带。
  const entryStateRef = useRef<unknown>(location.state)
  entryStateRef.current = location.state
  const openSession = useCallback(
    (id: string) => {
      if (!id || id === currentRef.current) return
      const replace = currentRef.current === NEW_SESSION_ROUTE_ID
      navigate(sessionPath(id), { replace, state: replace ? entryStateRef.current : undefined })
    },
    [navigate],
  )
  // 进入真实会话（非 'new'）时记下“上次停留的会话”；侧栏 / ⌥↑⌥↓ 切换同样经过这里。
  // 只记目录里确实存在的 id，错误链接不会顶掉上一条有效记录。
  const catalog = useSessionListQuery()
  const known = Boolean(
    sessionId &&
    sessionId !== NEW_SESSION_ROUTE_ID &&
    catalog.data?.sessions.some((session) => session.id === sessionId),
  )
  useEffect(() => {
    if (known) rememberLastSession(sessionId)
  }, [known, sessionId])
  const componentTabs = useSessionComponentTabs(context.notify, openSession)
  const pageChrome = useMemo(
    () => ({
      compact,
      overlayOnly: mobileApp,
      onOpenComponents: () => navigate(PAGE_PATHS.components),
      componentTabs,
      // 归档当前会话后回到主页：先清掉“上次停留”，主页随即显示新任务草稿，不再跳看板。
      onArchived: (id: string) => {
        forgetLastSession(id)
        navigate(HOME_PATH, { replace: true })
      },
      // “找不到这个会话”（已删除或链接错误）：回到主页（上次停留的会话或新任务草稿）。
      onHome: () => navigate(HOME_PATH),
    }),
    [compact, mobileApp, navigate, componentTabs],
  )
  // 手机外壳：同一份会话页契约，加上顶栏 ✎（没有改动时）打开派发表单。
  const startNewTask = context.startNewTask
  const phoneChrome = useMemo(
    () => ({ ...pageChrome, phone: { onNewTask: startNewTask } }),
    [pageChrome, startNewTask],
  )
  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 flex-col"
      data-session-shell={pageLayout ? 'desktop' : 'phone'}
    >
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
          // 手机外壳：☰（抽屉）放进会话顶栏最左侧；桌面布局的入口在应用侧栏里。
          routeLeading={pageLayout ? undefined : <PhoneMenuButton />}
          pageChrome={pageLayout ? pageChrome : phoneChrome}
        />
      </div>
    </div>
  )
}

// 组件标签：session-panel 的可渲染实例，按范围过滤到当前会话的项目；Runtime 没有原生组件能力时不提供（检查器只留提示）。
function useSessionComponentTabs(
  notify: (message: string) => void,
  openSession: (id: string) => void,
): SessionComponentTabs | undefined {
  const { language } = useI18n()
  const { available, placed } = useComponentInstances('session-panel')
  return useMemo(() => {
    if (!available) return undefined
    return {
      tabs: (cwd) => sessionPanelTabs(placed, cwd, language),
      render: (id, { sessionId, cwd }) => {
        const entry = placed.find((item) => item.instance.instanceId === id)
        return entry ? (
          <Suspense fallback={null}>
            <LazySessionPanelComponent
              placed={entry}
              sessionId={sessionId}
              cwd={cwd}
              notify={notify}
              onOpenSession={openSession}
            />
          </Suspense>
        ) : null
      },
      addMenu: ({ cwd, onAdded }) => (
        <Suspense fallback={null}>
          <LazySessionComponentAddMenu cwd={cwd} notify={notify} onAdded={onAdded} />
        </Suspense>
      ),
    }
  }, [available, language, notify, openSession, placed])
}
