// 会话主页（'/'，负责人决定“会话做主页”）：打开本设备上次停留的会话；它不存在、已归档或已删除时显示
// “新任务”草稿（会话页外壳 + 空转录 + 输入框），发出第一条消息时才建会话并以 replace 换成 /board/:id。
// 主页本身从不建会话（/board/new 一进入就建，主页回落不走那条路）。会话目录还在加载时显示加载态，
// 不把上次会话误判为不存在；目录读取失败时给出错误态与重试，不静默回落。
// 从主页落地的会话带 HOME_ENTRY_STATE：它是根，手机原生返回交还平台（见 shell-tabs.shellBackPath）。
// 本模块随会话路由懒加载，不进入入口包。
import { useCallback, useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useI18n } from '@/app/use-i18n'
import { PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { useSessionListQuery } from '@/features/board/public'
import { NewTaskDraft, PhoneAgentStrip } from '@/features/board/session-strip'
import { usePhoneShell, useTabletApp } from '@/hooks/usePhoneShell'
import { BoardEntry } from './BoardEntry'
import type { AppRouteContext } from './route-context'
import { HOME_ENTRY_STATE, PAGE_PATHS, sessionPath } from './routes'
import { homeLanding, readLastSession } from './session-home'

export function SessionHome({ context }: { context: AppRouteContext }) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const phone = usePhoneShell()
  const tablet = useTabletApp()
  const pageLayout = !phone || tablet
  // 落地时读一次：草稿期间别处写入的新记录不应把主页从草稿拽走。
  const [lastId] = useState(readLastSession)
  // 目录读取失败时用户可以明确选择直接开始新任务（不是静默回落）。
  const [draftChosen, setDraftChosen] = useState(false)
  const sessions = useSessionListQuery()
  const landing = draftChosen
    ? ({ kind: 'draft' } as const)
    : homeLanding({
        lastId,
        data: sessions.data,
        failed: sessions.isError && !sessions.isFetching,
      })
  const target = landing.kind === 'session' ? landing.id : ''

  const notify = context.notify
  // 草稿的第一条消息还在发送时用户可能已经离开主页（例如点了看板入口）。数据路由的 navigate 在卸载后
  // 照样生效，再 replace 就会把用户已在的页面换成新会话、还标成主页落地（手机返回键随即退出 App）。
  const mountedRef = useRef(false)
  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
    }
  }, [])

  // 主页里的会话入口（落地、手机代理条、草稿发出第一条消息）都替换当前历史条目并标为主页落地；
  // 桌面侧栏的会话行由应用壳按同样的规则处理（App.openSidebarSession）。
  // 主页已卸载时不再导航，改为全局提示会话已建好，并给出“打开会话”入口。
  const openHomeSession = useCallback(
    (id: string) => {
      if (!id) return
      if (mountedRef.current) {
        navigate(sessionPath(id), { replace: true, state: HOME_ENTRY_STATE })
        return
      }
      notify(t('board:launch.dispatched'), 'success', {
        action: { label: t('board:phone.openSession'), onAction: () => navigate(sessionPath(id)) },
      })
    },
    [navigate, notify, t],
  )
  useEffect(() => openHomeSession(target), [openHomeSession, target])
  const openBoard = useCallback(() => navigate(PAGE_PATHS.board), [navigate])
  const detachedFailure = useCallback((message: string) => notify(message, 'error'), [notify])
  const configureModels = useCallback(() => navigate('/config/models'), [navigate])
  // 手机顶栏的看板入口；桌面的入口在应用侧栏里。
  const entry = <BoardEntry onOpenBoard={openBoard} />

  let body
  if (landing.kind === 'draft')
    body = (
      <NewTaskDraft
        phone={!pageLayout}
        leading={entry}
        strip={
          pageLayout ? undefined : (
            <PhoneAgentStrip currentId="" notify={notify} onOpenSession={openHomeSession} />
          )
        }
        onDispatched={openHomeSession}
        onDetachedFailure={detachedFailure}
        onConfigureModels={configureModels}
      />
    )
  else
    body = (
      <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-panel">
        {!pageLayout && (
          <header className="flex h-14 shrink-0 items-center bg-chrome px-1">{entry}</header>
        )}
        <div className="flex min-h-0 flex-1 items-center justify-center">
          {landing.kind === 'error' ? (
            <PageState
              kind="error"
              title={t('board:home.loadFailed')}
              description={t('board:home.loadFailedHint')}
              action={
                <div className="flex flex-wrap justify-center gap-2">
                  <Button
                    variant="secondary"
                    className="pointer-coarse:min-h-11"
                    onClick={() => void sessions.refetch()}
                  >
                    {t('navigation:more.retry')}
                  </Button>
                  <Button
                    variant="ghost"
                    className="pointer-coarse:min-h-11"
                    onClick={() => setDraftChosen(true)}
                  >
                    {t('board:home.startDraft')}
                  </Button>
                </div>
              }
            />
          ) : (
            // 目录还在加载，或已确认上次会话、正在切过去：只显示加载态，不闪草稿也不跳看板。
            <PageState kind="loading" title={t('board:home.loading')} />
          )}
        </div>
      </div>
    )

  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 flex-col"
      data-session-shell={pageLayout ? 'desktop' : 'phone'}
      data-session-home={landing.kind}
    >
      <div className="flex min-h-0 min-w-0 flex-1">{body}</div>
    </div>
  )
}
