// 更新提示仅在确有可用更新时加载，避免给常驻导航增加入口体积。
import { Download, ExternalLink, RefreshCw, Rocket } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
export type SidebarUpdate = {
  info?: { desktop?: boolean; mobile?: boolean; version?: string }
  status?: {
    state: string
    percent?: number
    availableVersion?: string
    behindBy?: number
    branch?: string
    availableCommit?: string
  }
}

export default function SidebarUpdateStatus({
  update,
  collapsed,
  onOpen,
}: {
  update: SidebarUpdate
  collapsed: boolean
  onOpen: () => void
}) {
  const { t } = useI18n()
  const status = update?.status || { state: 'idle' }
  const nativeApp = Boolean(update?.info?.desktop || update?.info?.mobile)
  if (!['available', 'downloading', 'downloaded'].includes(status.state)) return null
  const downloading = status.state === 'downloading'
  const downloaded = status.state === 'downloaded'
  const label = downloaded
    ? t('navigation:appSidebar.readyToRestart')
    : downloading
      ? t('navigation:appSidebar.downloading')
      : nativeApp
        ? t('navigation:appSidebar.updateAvailable')
        : t('navigation:appSidebar.sourceUpdatesAvailable')
  const detail = downloading
    ? `${Math.round(status.percent || 0)}%`
    : nativeApp && status.availableVersion
      ? `v${status.availableVersion}`
      : status.behindBy
        ? t('navigation:appSidebar.countCommitsBehindBranch', {
            branch: status.branch || 'main',
            count: status.behindBy,
          })
        : status.availableCommit
          ? status.availableCommit.slice(0, 7)
          : t('navigation:appSidebar.viewUpdateDetails')
  const Icon = downloaded ? Rocket : downloading ? RefreshCw : nativeApp ? Download : ExternalLink

  return (
    <button
      type="button"
      className={`flex min-h-11 w-full items-center rounded-sm border border-star-border bg-star-soft text-content transition-colors duration-[var(--d-fast)] hover:bg-surface-hover ${collapsed ? 'justify-center px-0' : 'gap-2.5 px-3 text-left'}`}
      title={`${label} · ${detail}`}
      aria-label={`${label} · ${detail}`}
      onClick={onOpen}
    >
      <Icon className={`shrink-0 text-accent-fg ${downloading ? 'animate-spin' : ''}`} size={16} />
      {!collapsed && (
        <span className="min-w-0">
          <strong className="block truncate text-xs font-medium">{label}</strong>
          <small className="mt-0.5 block truncate text-xs text-content-muted tabular-nums">
            {detail}
          </small>
        </span>
      )}
    </button>
  )
}
