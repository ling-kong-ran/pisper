import { useEffect, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { Button } from '@/components/ui/button'
import { AppError } from '@/components/ui/app-primitives'
import { NOTIFICATION_PLATFORMS } from '@/types/notifications'
import type { NotificationPlatform, NotificationTemplate } from '@/types/notifications'
import type { useNotificationSettings } from './useNotificationSettings'
import type { Notify } from '@/app/route-context'
import type { I18nValues } from '@/app/i18n'
type Translate = (message: string, values?: I18nValues) => string
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

export function NotificationTemplatesEditor({
  settings,
  templates,
  eventId,
  platform,
  onSelectionChange,
  platformLabel,
  permissionGranted,
  onBrowserTest,
  notify,
}: {
  settings: ReturnType<typeof useNotificationSettings>
  templates: NotificationTemplate[]
  eventId: string
  platform: NotificationPlatform
  onSelectionChange: (event: string, platform: NotificationPlatform) => void
  platformLabel: (platform: NotificationPlatform) => string
  permissionGranted: boolean
  onBrowserTest: (title: string, body: string, tag?: string) => Promise<unknown>
  notify: Notify
}) {
  const { t } = useI18n()
  const alive = useRef(true)
  const [localError, setLocalError] = useState('')
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])
  const selected = templates.find((template) => template.id === eventId) || templates[0]
  if (!selected || !settings.data)
    return <p role="status">{t('config:notificationSettings.noNotificationEvents')}</p>
  const draft = settings.draft(selected.id, platform)
  const connection = platform === 'browser' ? null : settings.data.connections[platform]
  const canTest =
    platform === 'browser'
      ? settings.confirmed?.browser.enabled === true &&
        !settings.mutations.browser?.pending &&
        permissionGranted
      : connection?.enabled === true &&
        settings.data.scopes.some((scope) => scope.platform === platform)
  const test = async () => {
    setLocalError('')
    const result = await settings.testTemplate(selected.id, platform)
    if (!result || !alive.current) return
    try {
      if (platform === 'browser')
        await onBrowserTest(
          result.title || selected.name,
          result.body || result.preview || '',
          `pisper-template-${selected.id}`,
        )
      if (alive.current)
        notify(
          platform === 'browser'
            ? t(
                'config:notificationSettings.theTestNotificationWasSentCheckYourSystemNotificationCenter',
              )
            : t('config:notificationSettings.testNotificationSentToCountChats', {
                count: result.sent,
              }),
        )
    } catch (error) {
      if (alive.current) setLocalError(error instanceof Error ? error.message : String(error))
    }
  }
  return (
    <div className="space-y-4">
      <h2 className="text-[15px] leading-6 font-medium text-content">
        {t('config:notificationSettings.notificationTemplates')}
      </h2>
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="grid gap-1.5 text-sm font-medium text-content">
          {t('config:notificationSettings.notificationEvent')}
          <AppSelect
            aria-label={t('config:notificationSettings.notificationEvent')}
            value={selected.id}
            onChange={(event) => onSelectionChange(event.target.value, platform)}
          >
            {templates.map((template) => (
              <option key={template.id} value={template.id}>
                {template.name}
              </option>
            ))}
          </AppSelect>
        </label>
        <label className="grid gap-1.5 text-sm font-medium text-content">
          {t('config:notificationSettings.notificationChannel')}
          <AppSelect
            aria-label={t('config:notificationSettings.notificationChannel')}
            value={platform}
            onChange={(event) => {
              const channel = NOTIFICATION_PLATFORMS.find((item) => item === event.target.value)
              if (channel) onSelectionChange(selected.id, channel)
            }}
          >
            {NOTIFICATION_PLATFORMS.map((channel) => (
              <option key={channel} value={channel}>
                {platformLabel(channel)}
              </option>
            ))}
          </AppSelect>
        </label>
      </div>
      <p className="text-xs text-content-muted">{selected.description}</p>
      <label className="grid gap-1.5 text-sm font-medium text-content">
        {t('config:notificationSettings.messageContent')}
        <textarea
          aria-label={t('config:notificationSettings.messageContent')}
          value={draft.content}
          onChange={(event) => settings.editDraft(selected.id, platform, event.target.value)}
          className="min-h-36 w-full rounded-sm border border-input bg-panel p-3 font-mono text-[12.5px] leading-[1.65] font-normal caret-focus outline-none focus-visible:border-focus focus-visible:ring-3 focus-visible:ring-focus-ring"
        />
      </label>
      <div className="flex flex-wrap items-center gap-1.5 text-xs text-content-muted">
        <span>{t('config:notificationSettings.availableVariables')}</span>
        {selected.variables.map((variable) => (
          <code
            key={variable}
            className="rounded-xs bg-inline-code px-1.5 py-0.5 font-mono text-content-secondary"
          >{`{{${variable}}}`}</code>
        ))}
      </div>
      {/* 预览是凹陷的读数井：代码底色 + 细描边，与可编辑字段区分。 */}
      <div className="rounded-md border border-stroke bg-code p-3">
        <p className="text-xs font-medium text-content-muted">
          {t('config:notificationSettings.preview')}
        </p>
        <pre className="mt-2 font-sans text-sm break-words whitespace-pre-wrap text-content">
          {renderPreview(draft.content, t)}
        </pre>
      </div>
      <p className="text-xs text-content-muted">
        {draft.dirty
          ? t('config:notificationSettings.saveBeforeTest')
          : t('config:notificationSettings.testUsesSavedTemplate')}
      </p>
      {!canTest && (
        <p role="status" className="text-xs text-content-muted">
          {t('config:notificationSettings.channelTestUnavailable')}
        </p>
      )}
      {(draft.save.error || draft.test.error || localError) && (
        <AppError>{draft.save.error || draft.test.error || localError}</AppError>
      )}
      <div className="flex flex-wrap justify-end gap-2">
        <Button
          variant="secondary"
          className="max-[650px]:h-11"
          disabled={draft.dirty || draft.save.pending || !canTest}
          loading={draft.test.pending}
          onClick={() => void test()}
        >
          {t('config:notificationSettings.sendTest')}
        </Button>
        <Button
          data-template-save
          className="max-[650px]:h-11"
          loading={draft.save.pending}
          disabled={!draft.dirty || !draft.content.trim()}
          onClick={async () => {
            if (await settings.saveTemplate(selected.id, platform))
              notify(t('config:notificationSettings.notificationTemplateSaved'))
          }}
        >
          {t('config:notificationSettings.saveTemplate')}
        </Button>
      </div>
    </div>
  )
}
