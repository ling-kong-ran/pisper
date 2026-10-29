// 通知设置：浏览器通知开关 + 事件模板（飞书/微信/QQ/Telegram/浏览器渠道）配置。
import { useCallback, useEffect, useRef, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { Bell, BellOff, ChevronLeft, ShieldCheck } from 'lucide-react'
import {
  SettingsBadge as Badge,
  SettingsCard as Panel,
  SettingsGroup,
  SettingsSwitch as Toggle,
} from './settings-primitives'
import { APP_NAME } from '@/app/brand'
import { useI18n } from '@/app/use-i18n'
import { useNotificationSettings } from './useNotificationSettings'
import { NotificationTemplatesEditor } from './NotificationTemplatesEditor'
import { SettingsRow, SettingsSection } from '@/components/app/SettingsRow'
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuCheckboxItem,
  DropdownMenuItem,
  DropdownMenuSeparator,
} from '@/components/ui/dropdown-menu'
import { NOTIFICATION_PLATFORMS } from '@/types/notifications'
import {
  getBrowserNotificationPermission,
  prepareBrowserNotifications,
  requestBrowserNotificationPermission,
  showBrowserSystemNotification,
} from '@/lib/browser-notifications'
import {
  tauriNotificationAvailable,
  tauriNotificationIsGranted,
  tauriNotificationNotify,
  tauriNotificationRequestPermission,
} from '@/lib/tauri-notification'
import type { I18nValues } from '@/app/i18n'
import type { Notify } from '@/app/route-context'
import type { NotificationPlatform, NotificationSettingsData } from '@/types/notifications'

import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'

import { AppError, AppEmptyState, AppNotice } from '@/components/ui/app-primitives'

type Translate = (message: string, values?: I18nValues) => string
type NotificationPermissionState = NotificationPermission | 'checking' | 'unsupported'
type NotificationSettingsProps = {
  notify: Notify
  onBrowserNotificationChange?: (settings: NotificationSettingsData) => void
}

function notificationChannelLabel(
  platform: NotificationPlatform,
  t: ReturnType<typeof useI18n>['t'],
) {
  if (platform === 'feishu') return t('config:notificationSettings.feishu')
  if (platform === 'weixin') return t('config:notificationSettings.weChat')
  if (platform === 'qq') return t('config:notificationSettings.qq')
  if (platform === 'telegram') return t('config:notificationSettings.telegram')
  return t('config:notificationSettings.systemNotificationChannel')
}

function notificationTemplateName(id: string, fallback: string, t: Translate) {
  if (id === 'chat.completed') return t('config:notificationSettings.chatCompleted')
  if (id === 'chat.waiting') return t('config:notificationSettings.chatWaiting')
  if (id === 'schedule.completed') return t('config:notificationSettings.scheduleCompleted')
  if (id === 'schedule.failed') return t('config:notificationSettings.scheduleFailed')
  if (id === 'workflow.completed') return t('config:notificationSettings.workflowCompleted')
  if (id === 'workflow.failed') return t('config:notificationSettings.workflowFailed')
  return fallback
}

function notificationTemplateDescription(id: string, fallback: string, t: Translate) {
  if (id === 'chat.completed') return t('config:notificationSettings.chatCompletedDescription')
  if (id === 'chat.waiting') return t('config:notificationSettings.chatWaitingDescription')
  if (id === 'schedule.completed')
    return t('config:notificationSettings.scheduleCompletedDescription')
  if (id === 'schedule.failed') return t('config:notificationSettings.scheduleFailedDescription')
  if (id === 'workflow.completed')
    return t('config:notificationSettings.workflowCompletedDescription')
  if (id === 'workflow.failed') return t('config:notificationSettings.workflowFailedDescription')
  return fallback
}

function notificationPermission(): NotificationPermissionState {
  if (typeof window !== 'undefined' && window.pisperDesktop?.showNotification) {
    return window.pisperDesktop.getNotificationStatus ? 'checking' : 'granted'
  }
  // 移动端壳：权限判定走 Tauri 通知插件（Android WebView 没有 Notification API）。
  if (typeof window !== 'undefined' && tauriNotificationAvailable()) {
    return 'checking'
  }
  return getBrowserNotificationPermission()
}

function notificationFailureMessage(reason: unknown, t: Translate) {
  if (reason === 'system-disabled' || reason === 'app-disabled')
    return t(
      'config:notificationSettings.systemNotificationsAreOffAllowPisperToSendNotificationsInYourOperatingSystemSettings',
    )
  if (reason === 'unsupported')
    return t('config:notificationSettings.systemNotificationsAreNotSupportedInThisEnvironment')
  return t(
    'config:notificationSettings.theOperatingSystemDidNotAcceptTheNotificationCheckYourSystemNotificationSettings',
  )
}

function errorMessage(caught: unknown) {
  return caught instanceof Error ? caught.message : String(caught)
}

export function NotificationSettings({
  notify,
  onBrowserNotificationChange,
}: NotificationSettingsProps) {
  const { t, language } = useI18n()
  const permissionAlive = useRef(false)
  useEffect(() => {
    permissionAlive.current = true
    return () => {
      permissionAlive.current = false
    }
  }, [])
  const settings = useNotificationSettings(onBrowserNotificationChange)
  const { data, loading, loadError } = settings
  const [params, setParams] = useSearchParams()
  const desktop = Boolean(window.pisperDesktop?.showNotification)
  const tauriShell = tauriNotificationAvailable()
  const [permission, setPermission] = useState<NotificationPermissionState>(notificationPermission)
  const browserSaving = settings.mutations.browser?.pending === true

  // 刷新桌面通知权限：桥接能力缺失视为已授权，否则取真实状态；
  // 非法返回值按 supported 字段降级。
  const refreshDesktopPermission = useCallback(async () => {
    if (!desktop) return
    const getNotificationStatus = window.pisperDesktop?.getNotificationStatus
    if (!getNotificationStatus) {
      setPermission('granted')
      return
    }
    try {
      const result = await getNotificationStatus()
      if (!permissionAlive.current) return
      const nextPermission = result?.permission
      setPermission(
        nextPermission === 'default' ||
          nextPermission === 'denied' ||
          nextPermission === 'granted' ||
          nextPermission === 'checking' ||
          nextPermission === 'unsupported'
          ? nextPermission
          : result?.supported === false
            ? 'unsupported'
            : 'granted',
      )
    } catch {
      if (permissionAlive.current) setPermission('unsupported')
    }
  }, [desktop])

  useEffect(() => {
    if (!desktop) return undefined
    void refreshDesktopPermission()
    const refresh = () => {
      void refreshDesktopPermission()
    }
    window.addEventListener('focus', refresh)
    return () => window.removeEventListener('focus', refresh)
  }, [desktop, refreshDesktopPermission])

  // 移动端壳的权限刷新：Tauri 通知插件状态映射到与浏览器一致的权限模型，
  // 页面回前台时重新拉取。
  const refreshTauriPermission = useCallback(async () => {
    if (desktop || !tauriShell) return
    try {
      const granted = await tauriNotificationIsGranted()
      if (!permissionAlive.current) return
      setPermission(granted === true ? 'granted' : granted === false ? 'denied' : 'default')
    } catch {
      if (permissionAlive.current) setPermission('unsupported')
    }
  }, [desktop, tauriShell])
  useEffect(() => {
    if (desktop || !tauriShell) return undefined
    void refreshTauriPermission()
    const refresh = () => void refreshTauriPermission()
    window.addEventListener('focus', refresh)
    return () => window.removeEventListener('focus', refresh)
  }, [desktop, tauriShell, refreshTauriPermission])

  // 切换通知总开关：开关是服务端全局偏好（控制所有客户端与渠道的投递），
  // 不因当前环境权限缺失而回滚——本机收不到只给提示，其他已授权设备照常。
  // 开启时按环境尝试申请本机权限（桌面/移动壳/浏览器分别处理）。
  const updateBrowser = async (enabled: boolean) => {
    let localNote = ''
    if (enabled) {
      if (desktop && permission !== 'granted') {
        localNote = notificationFailureMessage('system-disabled', t)
      } else if (!desktop && tauriShell) {
        // 移动端壳：先查插件权限，未授权则运行时申请（Android 13+ 系统弹窗）。
        let nextPermission: NotificationPermissionState = 'denied'
        try {
          const granted = (await tauriNotificationIsGranted()) === true
          const resolved = granted ? 'granted' : await tauriNotificationRequestPermission()
          nextPermission =
            resolved === 'granted' ? 'granted' : resolved === 'denied' ? 'denied' : 'default'
        } catch {
          nextPermission = 'unsupported'
        }
        if (!permissionAlive.current) return
        setPermission(nextPermission)
        if (nextPermission !== 'granted') {
          localNote = t(
            'config:notificationSettings.systemNotificationsAreOffAllowPisperToSendNotificationsInYourOperatingSystemSettings',
          )
        }
      } else if (!desktop) {
        const nextPermission = await requestBrowserNotificationPermission()
        if (!permissionAlive.current) return
        setPermission(nextPermission)
        if (nextPermission !== 'granted') {
          localNote =
            nextPermission === 'unsupported'
              ? t('config:notificationSettings.systemNotificationsAreNotSupportedInThisEnvironment')
              : t(
                  'config:notificationSettings.notificationPermissionWasNotGrantedAllowNotificationsInTheBrowserSiteSettings',
                )
        } else {
          // Service Worker 注册失败只影响浏览器后台链路，不回滚全局开关。
          try {
            await prepareBrowserNotifications()
          } catch {
            localNote = t(
              'config:notificationSettings.theBrowserBackgroundNotificationServiceCouldNotBeRegistered',
            )
          }
        }
      }
    }
    if (!permissionAlive.current) return
    await settings.setBrowserEnabled(enabled)
    if (localNote && permissionAlive.current) notify(localNote, 'info')
  }

  // 发送系统通知（桌面桥接或浏览器），用于测试按钮；桌面返回
  // shown=false 时同步权限状态并抛错给出失败原因。
  const sendSystemNotification = useCallback(
    async (title: string, body: string, tag = 'pisper-browser-test') => {
      if (desktop) {
        const showNotification = window.pisperDesktop?.showNotification
        if (!showNotification)
          throw new Error(
            t('config:notificationSettings.systemNotificationsAreNotSupportedInThisEnvironment'),
          )
        const result = await showNotification({ title, body })
        if (result && typeof result === 'object' && result.shown === false) {
          setPermission(result.permission ?? 'denied')
          throw new Error(notificationFailureMessage(result.reason, t))
        }
        return result
      }
      // 移动端壳：经 Tauri 插件发本地通知。
      if (tauriShell) {
        await tauriNotificationNotify(title, body)
        return { shown: true, transport: 'tauri' }
      }
      return showBrowserSystemNotification({ title, body, tag, url: window.location.href })
    },
    [desktop, tauriShell, t],
  )

  // 测试通知：先校验权限（未授权给出对应提示），再发送一条测试通知。
  const testNotification = async () => {
    if (!settings.confirmed?.browser.enabled || browserSaving) return
    if (permission !== 'granted') {
      notify(
        permission === 'unsupported'
          ? notificationFailureMessage('unsupported', t)
          : desktop
            ? notificationFailureMessage('system-disabled', t)
            : t(
                'config:notificationSettings.notificationPermissionWasNotGrantedAllowNotificationsInTheBrowserSiteSettings',
              ),
        'error',
      )
      return
    }
    try {
      await sendSystemNotification(
        t('config:notificationSettings.appNotificationTest', { app: APP_NAME }),
        t('config:notificationSettings.notificationsAreWorking'),
      )
      notify(
        t(
          'config:notificationSettings.theTestNotificationWasSentCheckYourSystemNotificationCenter',
        ),
        'info',
      )
    } catch (caught) {
      notify(
        errorMessage(caught) ||
          t(
            'config:notificationSettings.theOperatingSystemDidNotAcceptTheNotificationCheckYourSystemNotificationSettings',
          ),
        'error',
      )
    }
  }

  const openDesktopNotificationSettings = async () => {
    try {
      const opened = await window.pisperDesktop?.openNotificationSettings?.()
      if (!opened)
        notify(
          t('config:notificationSettings.openNotificationPermissionsInYourOperatingSystemSettings'),
          'info',
        )
    } catch {
      notify(
        t('config:notificationSettings.openNotificationPermissionsInYourOperatingSystemSettings'),
        'info',
      )
    }
  }

  if (loading)
    return (
      <AppEmptyState>
        <OrbitSpinner className="size-6" />
        <h2>{t('config:notificationSettings.loadingNotificationSettings')}</h2>
      </AppEmptyState>
    )
  if (!data)
    return (
      <div className="space-y-3">
        <AppError>{loadError}</AppError>
        <Button variant="outline" onClick={() => void settings.load()}>
          {t('common:pageTemplate.retry')}
        </Button>
      </div>
    )
  const openTemplates = (
    event = data.templates[0]?.id || '',
    platform: NotificationPlatform = 'browser',
  ) => {
    setParams((current) => {
      const next = new URLSearchParams(current)
      next.set('view', 'templates')
      if (event) next.set('event', event)
      next.set('channel', platform)
      return next
    })
  }
  const showTemplates = params.get('view') === 'templates'
  const templates = data.templates.map((template) => ({
    ...template,
    name: notificationTemplateName(template.id, template.name, t),
    description: notificationTemplateDescription(template.id, template.description, t),
  }))
  if (showTemplates)
    return (
      <div data-config-card="notifications-templates" className="space-y-4">
        <Button
          variant="ghost"
          className="-ml-2.5 pointer-coarse:h-11"
          onClick={() =>
            setParams((current) => {
              const next = new URLSearchParams(current)
              next.delete('view')
              next.delete('event')
              next.delete('channel')
              return next
            })
          }
        >
          <ChevronLeft size={16} aria-hidden="true" />
          {t('config:notificationSettings.backToCategories')}
        </Button>
        <NotificationTemplatesEditor
          settings={settings}
          templates={templates}
          eventId={params.get('event') || ''}
          platform={
            NOTIFICATION_PLATFORMS.find((item) => item === params.get('channel')) || 'browser'
          }
          onSelectionChange={openTemplates}
          platformLabel={(platform) => notificationChannelLabel(platform, t)}
          permissionGranted={permission === 'granted'}
          onBrowserTest={sendSystemNotification}
          notify={notify}
        />
      </div>
    )
  const permissionLabel =
    permission === 'granted'
      ? t('config:notificationSettings.permissionGranted')
      : permission === 'denied'
        ? desktop
          ? t('config:notificationSettings.systemNotificationsAreOff')
          : t('config:notificationSettings.browserNotificationsAreBlocked')
        : permission === 'unsupported'
          ? t('config:notificationSettings.notSupported')
          : permission === 'checking'
            ? t('config:notificationSettings.checking')
            : t('config:notificationSettings.waitingForPermission')
  const permissionTone =
    permission === 'granted'
      ? 'green'
      : permission === 'default' || permission === 'checking'
        ? 'amber'
        : 'red'

  return (
    <div className="flex flex-col gap-6">
      {loadError && <AppError>{loadError}</AppError>}
      {Object.entries(settings.mutations)
        .filter(([, mutation]) => mutation.error)
        .map(([key, mutation]) => (
          <div key={key} className="flex flex-wrap items-center gap-2">
            <AppError>{mutation.error}</AppError>
            <Button
              variant="outline"
              size="sm"
              onClick={() => void settings.retryMutation(mutation)}
            >
              {t('common:pageTemplate.retry')}
            </Button>
          </div>
        ))}
      <Panel
        className="browser-notification-card p-4 py-4"
        data-config-card="notifications-browser"
      >
        {/* 通道总开关：键帽图标 + 标题/说明 + 权限读数徽标 + 开关；窄屏徽标与开关换到下一行。 */}
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <span
            aria-hidden="true"
            className={cn(
              'grid size-9.5 shrink-0 place-items-center rounded-lg border border-key-border bg-key shadow-key',
              data.browser.enabled ? 'text-accent-fg' : 'text-content-muted',
            )}
          >
            {data.browser.enabled ? <Bell size={18} /> : <BellOff size={18} />}
          </span>
          <div className="min-w-0 flex-1 basis-48">
            <p className="text-sm font-medium text-content">
              {t('config:notificationSettings.notification')}
            </p>
            <p className="mt-0.5 text-xs leading-[1.45] text-content-muted">
              {t(
                'config:notificationSettings.pisperUsesTheCurrentPlatformNotificationSystemWhenAnAgentCompletesOrFails',
              )}
            </p>
          </div>
          <div className="ml-auto flex items-center gap-3">
            <Badge tone={permissionTone}>{permissionLabel}</Badge>
            <Toggle
              ariaLabel={t('config:notificationSettings.notification')}
              value={data.browser.enabled}
              disabled={browserSaving || permission === 'checking'}
              onChange={updateBrowser}
            />
          </div>
        </div>
        <AppNotice>
          <ShieldCheck size={15} />
          <span>
            <strong>
              {desktop
                ? t('config:notificationSettings.controlledByOperatingSystemNotificationSettings')
                : t('config:notificationSettings.controlledByBrowserSitePermissions')}
            </strong>
            <small>
              {desktop
                ? t(
                    'config:notificationSettings.theDesktopAppUsesOperatingSystemNotificationsTurningThisOffDoesNotChangeTheOperatingSystemPermis',
                  )
                : t(
                    'config:notificationSettings.theWebAppUsesBrowserSiteNotificationsTurningThisOffDoesNotChangeTheBrowserPermissionItself',
                  )}
            </small>
          </span>
        </AppNotice>
        <div className="mt-4 flex gap-2 max-[650px]:flex-wrap">
          {desktop && permission === 'denied' && (
            <Button
              variant="secondary"
              className="max-[650px]:h-11"
              onClick={openDesktopNotificationSettings}
            >
              <ShieldCheck size={14} />
              {t('config:notificationSettings.openSystemNotificationSettings')}
            </Button>
          )}
          <Button
            variant="secondary"
            className="max-[650px]:h-11"
            disabled={!data.browser.enabled || browserSaving || permission !== 'granted'}
            onClick={testNotification}
          >
            <Bell size={14} />
            {t('config:notificationSettings.sendTestNotification')}
          </Button>
        </div>
      </Panel>
      <SettingsSection
        title={t('config:notificationSettings.notificationCategories')}
        description={t('config:notificationSettings.channelPreferenceHint')}
        action={
          <Button
            variant="ghost"
            size="sm"
            className="pointer-coarse:h-11"
            onClick={() => openTemplates()}
          >
            {t('config:notificationSettings.editNotificationTemplates')}
          </Button>
        }
      >
        {!settings.supportsChannels && (
          <p role="status" className="py-2 text-sm text-content-muted">
            {t('config:notificationSettings.channelPreferencesUnsupported')}
          </p>
        )}
        {/* 事件行装入同一张细线卡；空列表时不渲染空卡框。 */}
        {templates.length > 0 && (
          <SettingsGroup>
            {templates.map((template) => {
              const selected = template.enabled
                ? NOTIFICATION_PLATFORMS.filter(
                    (platform) => template.channels[platform]?.enabled !== false,
                  )
                : []
              const pending = Object.entries(settings.mutations).some(
                ([key, mutation]) =>
                  mutation.pending &&
                  (key === `event:${template.id}` || key.startsWith(`channel:${template.id}:`)),
              )
              return (
                <SettingsRow
                  key={template.id}
                  data-notification-event={template.id}
                  title={template.name}
                  description={template.description}
                  control={
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button
                          variant="outline"
                          size="sm"
                          className="max-w-full font-normal text-content-secondary pointer-coarse:h-11"
                          aria-label={t('config:notificationSettings.eventChannels', {
                            event: template.name,
                          })}
                          aria-busy={pending}
                        >
                          <span className="max-w-64 truncate">
                            {selected.length
                              ? selected
                                  .map((platform) => notificationChannelLabel(platform, t))
                                  .join(language === 'zh-CN' ? '、' : ', ')
                              : t('config:notificationSettings.channelsOff')}
                          </span>
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent
                        align="end"
                        className="max-w-[calc(100vw-40px)] min-w-56"
                      >
                        <DropdownMenuCheckboxItem
                          checked={template.enabled}
                          onCheckedChange={(enabled) =>
                            void settings.setEventEnabled(template.id, enabled === true)
                          }
                        >
                          {t('config:notificationSettings.enableEvent')}
                        </DropdownMenuCheckboxItem>
                        <DropdownMenuSeparator />
                        {NOTIFICATION_PLATFORMS.map((platform) => {
                          const available =
                            platform === 'browser'
                              ? data.browser.enabled
                              : data.connections[platform]?.enabled === true &&
                                data.scopes.some((scope) => scope.platform === platform)
                          return (
                            <DropdownMenuCheckboxItem
                              key={platform}
                              checked={template.channels[platform]?.enabled !== false}
                              disabled={!settings.supportsChannels}
                              onCheckedChange={(enabled) =>
                                void settings.setChannelEnabled(
                                  template.id,
                                  platform,
                                  enabled === true,
                                )
                              }
                            >
                              <span>{notificationChannelLabel(platform, t)}</span>
                              {!available && (
                                <small className="ml-2 text-muted-foreground">
                                  {t('config:notificationSettings.channelNeedsSetup')}
                                </small>
                              )}
                            </DropdownMenuCheckboxItem>
                          )
                        })}
                        <DropdownMenuSeparator />
                        <DropdownMenuItem onSelect={() => openTemplates(template.id)}>
                          {t('config:notificationSettings.editNotificationTemplates')}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  }
                />
              )
            })}
          </SettingsGroup>
        )}
        {!templates.length && (
          <p role="status" className="py-3 text-sm text-content-muted">
            {t('config:notificationSettings.noNotificationEvents')}
          </p>
        )}
      </SettingsSection>
    </div>
  )
}
