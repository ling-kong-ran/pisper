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
      <h2 className="text-lg font-medium">
        {t('config:notificationSettings.notificationTemplates')}
      </h2>
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="grid gap-1 text-sm">
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
        <label className="grid gap-1 text-sm">
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
      <p className="text-sm text-muted-foreground">{selected.description}</p>
      <label className="grid gap-2 text-sm">
        {t('config:notificationSettings.messageContent')}
        <textarea
          aria-label={t('config:notificationSettings.messageContent')}
          value={draft.content}
          onChange={(event) => settings.editDraft(selected.id, platform, event.target.value)}
          className="min-h-36 w-full rounded-lg border border-border bg-background p-3 font-mono text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
        />
      </label>
      <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
        <span>{t('config:notificationSettings.availableVariables')}</span>
        {selected.variables.map((variable) => (
          <code key={variable}>{`{{${variable}}}`}</code>
        ))}
      </div>
      <div className="rounded-lg border border-border p-3">
        <p className="text-xs text-muted-foreground">{t('config:notificationSettings.preview')}</p>
        <pre className="mt-2 whitespace-pre-wrap break-words font-sans text-sm">
          {renderPreview(draft.content, t)}
        </pre>
      </div>
      <p className="text-xs text-muted-foreground">
        {draft.dirty
          ? t('config:notificationSettings.saveBeforeTest')
          : t('config:notificationSettings.testUsesSavedTemplate')}
      </p>
      {!canTest && (
        <p role="status" className="text-xs text-muted-foreground">
          {t('config:notificationSettings.channelTestUnavailable')}
        </p>
      )}
      {(draft.save.error || draft.test.error || localError) && (
        <AppError>{draft.save.error || draft.test.error || localError}</AppError>
      )}
      <div className="flex flex-wrap justify-end gap-2">
        <Button
          variant="outline"
          disabled={draft.dirty || draft.save.pending || !canTest}
          loading={draft.test.pending}
          onClick={() => void test()}
        >
          {t('config:notificationSettings.sendTest')}
        </Button>
        <Button
          data-template-save
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
