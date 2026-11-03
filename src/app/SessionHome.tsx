// 会话主页（'/'，负责人决定“会话做主页”）：打开本设备上次停留的会话；它不存在、已归档或已删除时
// 进入新任务草稿 '/board/new'（会话页的样子，发出第一条消息时才建会话）。主页本身从不建会话。
// 会话目录还在加载时显示加载态，不把上次会话误判为不存在；目录读取失败时给出错误态与重试，不静默回落。
// 从主页落地的会话与草稿带 HOME_ENTRY_STATE：它是根，手机原生返回交还平台（见 shell-tabs.shellBackPath）。
// 本模块随会话路由懒加载，不进入入口包。
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useI18n } from '@/app/use-i18n'
import { PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { useSessionListQuery } from '@/features/board/public'
import { usePhoneShell, useTabletApp } from '@/hooks/usePhoneShell'
import { PhoneMenuButton } from './PhoneMenuButton'
import type { AppRouteContext } from './route-context'
import { HOME_ENTRY_STATE, sessionPath } from './routes'
import { homeLanding, readLastSession } from './session-home'

export function SessionHome(_props: { context: AppRouteContext }) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const phone = usePhoneShell()
  const tablet = useTabletApp()
  const pageLayout = !phone || tablet
  // 落地时读一次：别处写入的新记录不应把主页从这次落地拽走。
  const [lastId] = useState(readLastSession)
  const sessions = useSessionListQuery()
  const landing = homeLanding({
    lastId,
    data: sessions.data,
    failed: sessions.isError && !sessions.isFetching,
  })
  // 落地（上次停留的会话或新任务草稿）都替换当前历史条目并标为主页落地；
  // 桌面侧栏的会话行由应用壳按同样的规则处理（App.openSidebarSession）。
  const target =
    landing.kind === 'session'
      ? sessionPath(landing.id)
      : landing.kind === 'draft'
        ? sessionPath()
        : ''
  useEffect(() => {
    if (target) navigate(target, { replace: true, state: HOME_ENTRY_STATE })
  }, [navigate, target])
  // 手机顶栏左侧的 ☰（抽屉）；桌面的入口在应用侧栏里。
  const entry = <PhoneMenuButton />

  const body = (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-panel">
      {!pageLayout && (
        <header className="flex h-14 shrink-0 items-center bg-panel px-1">{entry}</header>
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
                  onClick={() => navigate(sessionPath(), { state: HOME_ENTRY_STATE })}
                >
                  {t('board:home.startDraft')}
                </Button>
              </div>
            }
          />
        ) : (
          // 目录还在加载，或已确认落地、正在切过去：只显示加载态，不闪草稿也不跳看板。
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
