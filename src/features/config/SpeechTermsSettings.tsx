import { useEffect, useId, useState } from 'react'
import { AlertTriangle, ChevronDown, RefreshCw, Save } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { AppError } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { apiJson } from '@/lib/api'
import { SettingsSwitch } from './settings-primitives'

type SpeechTermsConfig = {
  projectTermsEnabled: boolean
  customTerms: string[]
  builtinTerms: string[]
}

export function SpeechTermsSettings({ notify }: { notify: Notify }) {
  const { t } = useI18n()
  const id = useId()
  const [settings, setSettings] = useState<SpeechTermsConfig | null>(null)
  const [projectTermsEnabled, setProjectTermsEnabled] = useState(false)
  const [termsText, setTermsText] = useState('')
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [loadAttempt, setLoadAttempt] = useState(0)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setError('')
    void apiJson<SpeechTermsConfig>('/api/settings/speech')
      .then((data) => {
        if (cancelled) return
        setSettings(data)
        setProjectTermsEnabled(data.projectTermsEnabled)
        setTermsText(data.customTerms.join('\n'))
      })
      .catch((caught: unknown) => {
        if (!cancelled) setError(caught instanceof Error ? caught.message : String(caught))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [loadAttempt])

  const customTerms = termsText
    .split(/\r?\n/)
    .map((term) => term.trim())
    .filter(Boolean)
  const validationError =
    customTerms.length > 64
      ? t('config:speechTermsSettings.tooManyTerms')
      : customTerms.some((term) => term.length > 64)
        ? t('config:speechTermsSettings.termTooLong')
        : ''
  const disabled = loading || saving || !settings
  const dirty =
    settings !== null &&
    (projectTermsEnabled !== settings.projectTermsEnabled ||
      termsText !== settings.customTerms.join('\n'))

  const save = async () => {
    if (disabled || !dirty || validationError) return
    setSaving(true)
    setError('')
    try {
      const saved = await apiJson<SpeechTermsConfig>('/api/settings/speech', {
        method: 'PATCH',
        body: JSON.stringify({ projectTermsEnabled, customTerms }),
      })
      setSettings(saved)
      setProjectTermsEnabled(saved.projectTermsEnabled)
      setTermsText(saved.customTerms.join('\n'))
      notify(t('config:speechTermsSettings.saved'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  return (
    <section
      data-config-card="models-speech"
      aria-labelledby={`${id}-title`}
      aria-busy={loading || saving}
      className="my-3 min-w-0 border-t border-border py-3"
    >
      <form
        className="flex min-w-0 flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault()
          void save()
        }}
      >
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h2 id={`${id}-title`} className="text-sm font-semibold text-content">
            {t('config:speechTermsSettings.title')}
          </h2>
          <Button type="submit" size="sm" disabled={disabled || !dirty || !!validationError}>
            {saving ? <RefreshCw size={13} className="animate-spin" /> : <Save size={13} />}
            {saving ? t('config:configPage.saving') : t('config:configPage.saveSettings')}
          </Button>
        </div>
        {loading && (
          <div role="status" className="flex items-center gap-2 text-xs text-content-muted">
            <RefreshCw size={13} className="animate-spin" />
            {t('config:configPage.loading')}
          </div>
        )}
        <div className="flex items-center justify-between gap-3">
          <Label htmlFor={`${id}-project`} className="min-w-0 text-sm">
            {t('config:speechTermsSettings.projectTerms')}
          </Label>
          <SettingsSwitch
            id={`${id}-project`}
            value={projectTermsEnabled}
            disabled={disabled}
            className="after:inset-x-0"
            onChange={setProjectTermsEnabled}
          />
        </div>
        <div className="flex min-w-0 flex-col gap-2">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Label htmlFor={`${id}-terms`} className="min-w-0 text-sm">
              {t('config:speechTermsSettings.customTerms')}
            </Label>
            <span id={`${id}-count`} className="text-xs tabular-nums text-content-muted">
              {t('config:speechTermsSettings.termCount', { count: customTerms.length })}
            </span>
          </div>
          <Textarea
            id={`${id}-terms`}
            value={termsText}
            onChange={(event) => setTermsText(event.target.value)}
            disabled={disabled}
            rows={4}
            spellCheck={false}
            aria-invalid={!!validationError}
            aria-describedby={`${id}-count${validationError ? ` ${id}-validation` : ''}`}
            className="h-28 min-h-28 resize-y [field-sizing:fixed]"
          />
          {validationError && <AppError id={`${id}-validation`}>{validationError}</AppError>}
        </div>
        {settings && settings.builtinTerms.length > 0 && (
          <Collapsible>
            <CollapsibleTrigger asChild>
              <Button type="button" variant="ghost" size="sm" className="group max-w-full">
                <ChevronDown
                  size={13}
                  className="shrink-0 transition-transform group-data-[state=closed]:-rotate-90"
                />
                <span className="min-w-0 whitespace-normal text-left">
                  {t('config:speechTermsSettings.builtinTerms', {
                    count: settings.builtinTerms.length,
                  })}
                </span>
              </Button>
            </CollapsibleTrigger>
            <CollapsibleContent>
              <p className="mt-2 max-h-28 overflow-y-auto text-xs leading-5 break-words text-content-muted">
                {settings.builtinTerms.join(', ')}
              </p>
            </CollapsibleContent>
          </Collapsible>
        )}
        {error && (
          <AppError>
            <AlertTriangle size={13} className="shrink-0" />
            <span className="min-w-0 break-words">{error}</span>
          </AppError>
        )}
        {!settings && !loading && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="self-start"
            onClick={() => setLoadAttempt((attempt) => attempt + 1)}
          >
            <RefreshCw size={13} />
            {t('config:speechTermsSettings.retry')}
          </Button>
        )}
      </form>
    </section>
  )
}
