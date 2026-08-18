// 通知设置：浏览器通知开关 + 事件模板（飞书/微信/浏览器渠道）配置。
import { useCallback, useEffect, useState } from 'react'
import { AlertTriangle, Bell, BellOff, RefreshCw, Save, Send, ShieldCheck } from 'lucide-react'
import {
  SettingsBadge as Badge,
  SettingsCard as Panel,
  SettingsSectionTitle as SectionTitle,
  SettingsSwitch as Toggle,
} from './settings-primitives'
import { APP_NAME } from '@/app/brand'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import {
  getBrowserNotificationPermission,
  prepareBrowserNotifications,
  requestBrowserNotificationPermission,
  showBrowserSystemNotification,
} from '@/lib/browser-notifications'
import type { Dispatch, SetStateAction } from 'react'
import type { I18nValues } from '@/app/i18n'
import type { Notify } from '@/app/route-context'
import type { NotificationPlatform, NotificationSettingsData } from '@/types/notifications'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

import { AppCardHeader, AppError, AppEmptyState, AppNotice } from '@/components/ui/app-primitives'

type Translate = (message: string, values?: I18nValues) => string
type NotificationPermissionState = NotificationPermission | 'checking' | 'unsupported'
type ChannelDefinition = {
  tone: 'blue' | 'green'
}
type NotificationTestResult = {
  sent?: number
  title?: string
  body?: string
  preview?: string
}
type NotificationSettingsProps = {
  notify: Notify
  onBrowserNotificationChange?: (settings: NotificationSettingsData) => void
}
type NotificationTemplatesProps = {
  data: NotificationSettingsData
  setData: Dispatch<SetStateAction<NotificationSettingsData>>
  notify: Notify
  permission: NotificationPermissionState
  onBrowserTest?: (title: string, body: string, tag?: string) => Promise<unknown>
  onSettingsChange?: (settings: NotificationSettingsData) => void
}

const CHANNELS: Record<NotificationPlatform, ChannelDefinition> = {
  feishu: { tone: 'blue' },
  weixin: { tone: 'green' },
  browser: { tone: 'blue' },
}

function notificationChannelLabel(
  platform: NotificationPlatform,
  t: ReturnType<typeof useI18n>['t'],
) {
  if (platform === 'feishu') return t('config:notificationSettings.feishu')
  if (platform === 'weixin') return t('config:notificationSettings.weChat')
  return t('config:notificationSettings.browserNotification')
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

function renderPreview(content: string, t: Translate) {
  const values: Record<string, string> = {
    'chat.title': t('config:notificationSettings.fixChannelNotification'),
    'chat.summary': t(
      'config:notificationSettings.implementationIsCompleteTestsAndBuildHavePassed',
    ),
    'chat.tool': 'bash',
    'chat.reason': t('config:notificationSettings.confirmationIsRequired'),
    'chat.model': 'openai/gpt-5.4',
    'task.name': t('config:notificationSettings.dailyCodeReview'),
    'task.summary': t('config:notificationSettings.found2IssuesToAddressTheReportHasBeenArchived'),
    'task.duration': t('config:notificationSettings.message2Min18Sec'),
    'task.nextRun': t('config:notificationSettings.tomorrow0900'),
    'task.error': t('config:notificationSettings.testProcessTimedOut'),
    'workflow.name': t('config:notificationSettings.preReleaseChecks'),
    'workflow.summary': t('config:notificationSettings.testsBuildAndSecurityChecksHaveAllPassed'),
    'workflow.duration': t('config:notificationSettings.message6Min42Sec'),
    'workflow.runId': 'run_20260718_001',
    'workflow.node': t('config:notificationSettings.endToEndTests'),
    'workflow.error': t('config:notificationSettings.browserFailedToStart'),
  }
  return String(content || '').replace(
    /\{\{\s*([\w.]+)\s*\}\}/g,
    (_match, key) => values[key] || `{{${key}}}`,
  )
}

function notificationPermission(): NotificationPermissionState {
  if (typeof window !== 'undefined' && window.pisperDesktop?.showNotification) {
    return window.pisperDesktop.getNotificationStatus ? 'checking' : 'granted'
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
  const { t } = useI18n()
  const [data, setData] = useState<NotificationSettingsData>({
    browser: { enabled: false },
    connections: {},
    scopes: [],
    templates: [],
  })
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const desktop = Boolean(window.pisperDesktop?.showNotification)
  const [permission, setPermission] = useState<NotificationPermissionState>(notificationPermission)
  const [browserSaving, setBrowserSaving] = useState(false)

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
      setPermission('unsupported')
    }
  }, [desktop])

  // 加载通知设置并同步给壳层（供 App 的浏览器通知逻辑使用）。
  const load = useCallback(async () => {
    try {
      setError('')
      const result = await apiJson<NotificationSettingsData>('/api/settings/notifications')
      setData(result)
      onBrowserNotificationChange?.(result)
    } catch (caught) {
      setError(errorMessage(caught))
    } finally {
      setLoading(false)
    }
  }, [onBrowserNotificationChange])

  useEffect(() => {
    load()
  }, [load])
  useEffect(() => {
    if (!desktop) return undefined
    void refreshDesktopPermission()
    const refresh = () => {
      void refreshDesktopPermission()
    }
    window.addEventListener('focus', refresh)
    return () => window.removeEventListener('focus', refresh)
  }, [desktop, refreshDesktopPermission])

  // 切换浏览器通知开关：开启前校验环境支持与权限（桌面/浏览器分别处理），
  // 未授权时提示并回滚开关。
  const updateBrowser = async (enabled: boolean) => {
    if (enabled) {
      if (permission === 'unsupported') {
        notify(
          t('config:notificationSettings.systemNotificationsAreNotSupportedInThisEnvironment'),
          'error',
        )
        return
      }
      if (desktop && permission !== 'granted') {
        notify(notificationFailureMessage('system-disabled', t), 'error')
        return
      }
      if (!desktop) {
        const nextPermission = await requestBrowserNotificationPermission()
        setPermission(nextPermission)
        if (nextPermission !== 'granted') {
          notify(
            t(
              'config:notificationSettings.notificationPermissionWasNotGrantedAllowNotificationsInTheBrowserSiteSettings',
            ),
            'error',
          )
          return
        }
        try {
          await prepareBrowserNotifications()
        } catch (caught) {
          notify(
            errorMessage(caught) ||
              t(
                'config:notificationSettings.theBrowserBackgroundNotificationServiceCouldNotBeRegistered',
              ),
            'error',
          )
          return
        }
      }
    }
    setBrowserSaving(true)
    try {
      const result = await apiJson<NotificationSettingsData>(
        '/api/settings/notifications/browser',
        {
          method: 'PATCH',
          body: JSON.stringify({ enabled }),
        },
      )
      setData(result)
      onBrowserNotificationChange?.(result)
      notify(
        enabled
          ? t('config:notificationSettings.notificationsEnabled')
          : t('config:notificationSettings.notificationsDisabled'),
      )
    } catch (caught) {
      notify(errorMessage(caught), 'error')
    } finally {
      setBrowserSaving(false)
    }
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
      return showBrowserSystemNotification({ title, body, tag, url: window.location.href })
    },
    [desktop, t],
  )

  // 测试通知：先校验权限（未授权给出对应提示），再发送一条测试通知。
  const testNotification = async () => {
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
        <RefreshCw className="animate-spin" size={23} />
        <h2>{t('config:notificationSettings.loadingNotificationSettings')}</h2>
      </AppEmptyState>
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
    <div className="flex flex-col gap-[12px]">
      {error && (
        <AppError>
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
      <Panel className="browser-notification-card">
        <div className="notification-option [&_>_div]:flex [&_>_div]:min-w-0 [&_>_div]:flex-col [&_>_div]:gap-[4px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] [&_small]:leading-[1.45] grid grid-cols-[auto_minmax(0,1fr)_auto_auto] items-center gap-[11px]">
          <span
            className={`provider-icon [&_svg]:w-[19px] [&.green]:bg-[var(--success-soft)] [&.green]:text-[var(--success)] [&.blue]:bg-[var(--brand-blue-soft)] [&.blue]:text-[var(--brand-blue-strong)] [.notification-option_&:not(.blue)]:bg-[var(--surface-muted)] [.notification-option_&:not(.blue)]:text-[var(--text-muted)] grid w-[38px] h-[38px] place-items-center rounded-[var(--r-md)] ${data.browser.enabled ? 'blue' : ''}`}
          >
            {data.browser.enabled ? <Bell size={18} /> : <BellOff size={18} />}
          </span>
          <div>
            <strong>{t('config:notificationSettings.notification')}</strong>
            <small>
              {t(
                'config:notificationSettings.pisperUsesTheCurrentPlatformNotificationSystemWhenAnAgentCompletesOrFails',
              )}
            </small>
          </div>
          <Badge tone={permissionTone}>{permissionLabel}</Badge>
          <Toggle
            value={data.browser.enabled}
            disabled={browserSaving || permission === 'unsupported' || permission === 'checking'}
            onChange={updateBrowser}
          />
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
        <div className="mt-[15px] flex gap-2 max-[650px]:flex-wrap">
          {desktop && permission === 'denied' && (
            <Button
              variant="outline"
              size="lg"
              className="bg-surface-subtle"
              onClick={openDesktopNotificationSettings}
            >
              <ShieldCheck size={14} />
              {t('config:notificationSettings.openSystemNotificationSettings')}
            </Button>
          )}
          <Button
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            disabled={!data.browser.enabled || permission !== 'granted'}
            onClick={testNotification}
          >
            <Bell size={14} />
            {t('config:notificationSettings.sendTestNotification')}
          </Button>
        </div>
      </Panel>
      <NotificationTemplates
        data={data}
        setData={setData}
        notify={notify}
        permission={permission}
        onBrowserTest={sendSystemNotification}
        onSettingsChange={onBrowserNotificationChange}
      />
    </div>
  )
}

function NotificationTemplates({
  data,
  setData,
  notify,
  permission,
  onBrowserTest,
  onSettingsChange,
}: NotificationTemplatesProps) {
  const { t } = useI18n()
  const [eventId, setEventId] = useState(data.templates[0]?.id || '')
  const [platform, setPlatform] = useState<NotificationPlatform>('feishu')
  const selected = data.templates.find((item) => item.id === eventId) || data.templates[0]
  const variant = selected?.channels?.[platform]
  const [content, setContent] = useState(variant?.content || '')
  const [saving, setSaving] = useState(false)
  const latestScope = data.scopes.find((scope) => scope.platform === platform)
  const canTest =
    platform === 'browser' ? data.browser.enabled && permission === 'granted' : Boolean(latestScope)
  const visibleChannels = CHANNELS

  useEffect(() => {
    setContent(variant?.content || '')
  }, [eventId, platform, variant?.content])
  if (!selected) return null

  const save = async () => {
    setSaving(true)
    try {
      const result = await apiJson<NotificationSettingsData>(
        `/api/settings/notifications/templates/${encodeURIComponent(selected.id)}/${platform}`,
        { method: 'PUT', body: JSON.stringify({ enabled: selected.enabled, content }) },
      )
      setData(result)
      onSettingsChange?.(result)
      notify(t('config:notificationSettings.notificationTemplateSaved'))
    } catch (caught) {
      notify(errorMessage(caught), 'error')
    } finally {
      setSaving(false)
    }
  }

  const test = async () => {
    setSaving(true)
    try {
      const result = await apiJson<NotificationTestResult>(
        `/api/settings/notifications/templates/${encodeURIComponent(selected.id)}/${platform}/test`,
        { method: 'POST', body: '{}' },
      )
      if (platform === 'browser')
        await onBrowserTest?.(
          result.title || notificationTemplateName(selected.id, selected.name, t),
          result.body || result.preview || '',
          `pisper-template-${selected.id}`,
        )
      notify(
        platform === 'browser'
          ? t(
              'config:notificationSettings.theTestNotificationWasSentCheckYourSystemNotificationCenter',
              { count: result.sent },
            )
          : t('config:notificationSettings.testNotificationSentToCountChats', {
              count: result.sent,
            }),
      )
    } catch (caught) {
      notify(errorMessage(caught), 'error')
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="two-one-grid max-[900px]:grid-cols-[1fr] grid grid-cols-[minmax(0,2fr)_minmax(260px,1fr)] gap-[12px] [margin-top:0]">
      <Panel>
        <div className="channel-section-head [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[13px] flex items-center justify-between gap-[8px] [margin-bottom:8px]">
          <SectionTitle title={t('config:notificationSettings.notificationTemplates')} />
          <span>{t('config:notificationSettings.sharedByChatsScheduledTasksAndWorkflows')}</span>
        </div>
        {data.templates.map((template) => (
          <button
            className={`channel-template-row hover:rounded-[var(--r-sm)] hover:bg-[var(--accent-soft)] [&.selected]:rounded-[var(--r-sm)] [&.selected]:bg-[var(--accent-soft)] [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[4px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] grid w-full min-h-[58px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[9px] border-0 [border-top:1px_solid_var(--stroke-soft)] bg-transparent [padding:8px_6px] text-left ${template.id === selected.id ? 'selected' : ''}`}
            onClick={() => setEventId(template.id)}
            key={template.id}
          >
            <span className="route-icon grid w-[27px] h-[27px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)]">
              <Send size={14} />
            </span>
            <span>
              <strong>{notificationTemplateName(template.id, template.name, t)}</strong>
              <small>{notificationTemplateDescription(template.id, template.description, t)}</small>
            </span>
            <Badge tone={template.enabled ? 'green' : 'gray'}>
              {template.enabled
                ? t('config:notificationSettings.enabled')
                : t('config:notificationSettings.disabled')}
            </Badge>
          </button>
        ))}
      </Panel>
      <Panel className="channel-template-editor [&_textarea]:min-h-[118px] [&_textarea]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_textarea]:leading-[1.55]">
        <AppCardHeader>
          <div>
            <h2>{notificationTemplateName(selected.id, selected.name, t)}</h2>
            <p>{notificationTemplateDescription(selected.id, selected.description, t)}</p>
          </div>
          <Toggle
            value={selected.enabled}
            onChange={(enabled) =>
              setData((current) => ({
                ...current,
                templates: current.templates.map((item) =>
                  item.id === selected.id ? { ...item, enabled } : item,
                ),
              }))
            }
          />
        </AppCardHeader>
        <div className="channel-template-platforms [&_>_button]:flex [&_>_button]:min-h-[36px] [&_>_button]:items-center [&_>_button]:justify-between [&_>_button]:gap-[7px] [&_>_button]:[border:1px_solid_var(--stroke)] [&_>_button]:rounded-[var(--r-sm)] [&_>_button]:bg-[var(--surface-subtle)] [&_>_button]:p-[0_9px] [&_>_button]:text-[12px] [&_>_button]:font-[700] [&_>_button.active]:border-[var(--focus)] [&_>_button.active]:bg-[var(--accent-soft)] [&_>_button.active]:text-[var(--star-strong)] max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(3,minmax(0,1fr))] gap-[7px] [margin-top:13px]">
          {(
            Object.entries(visibleChannels) as Array<[NotificationPlatform, ChannelDefinition]>
          ).map(([id, channel]) => {
            const available = id === 'browser' ? data.browser.enabled : data.connections?.[id]
            return (
              <button
                className={platform === id ? 'active' : ''}
                onClick={() => setPlatform(id)}
                key={id}
              >
                {notificationChannelLabel(id, t)}
                <Badge tone={available ? channel.tone : 'gray'}>
                  {available
                    ? id === 'browser'
                      ? t('config:notificationSettings.enabled2')
                      : t('config:notificationSettings.connected')
                    : id === 'browser'
                      ? t('config:notificationSettings.notEnabled')
                      : t('config:notificationSettings.notConnected')}
                </Badge>
              </button>
            )
          })}
        </div>
        <FieldLabel variant="control">
          {t('config:notificationSettings.messageContent')}
          <textarea value={content} onChange={(event) => setContent(event.target.value)} />
        </FieldLabel>
        <div className="channel-template-vars [&_>_span]:mr-[3px] [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[13px] [&_code]:rounded-[var(--r-xs)] [&_code]:bg-[var(--template-code-bg)] [&_code]:text-[var(--template-code-text)] [&_code]:p-[3px_5px] [&_code]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_code]:text-[13px] flex flex-wrap items-center gap-[5px] [margin-top:10px]">
          <span>{t('config:notificationSettings.availableVariables')}</span>
          {selected.variables.map((variable) => (
            <code key={variable}>{`{{${variable}}}`}</code>
          ))}
        </div>
        <div className="channel-template-preview [&_>_small]:text-[var(--text-muted)] [&_>_small]:text-[13px] [&_pre]:m-[7px_0_0] [&_pre]:overflow-auto [&_pre]:text-[var(--text-soft)] [&_pre]:font-[inherit] [&_pre]:text-[12px] [&_pre]:leading-[1.55] [&_pre]:whitespace-pre-wrap [margin-top:10px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:9px]">
          <small>{t('config:notificationSettings.preview')}</small>
          <pre>{renderPreview(content, t)}</pre>
        </div>
        <div className="flex justify-end gap-[8px] [margin-top:18px]">
          <Button
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            disabled={saving || !canTest}
            onClick={test}
          >
            <Send size={14} />
            {t('config:notificationSettings.sendTest')}
          </Button>
          <Button size="lg" disabled={saving || !content.trim()} onClick={save}>
            {saving ? <RefreshCw className="animate-spin" size={14} /> : <Save size={14} />}
            {t('config:notificationSettings.saveTemplate')}
          </Button>
        </div>
      </Panel>
    </div>
  )
}
