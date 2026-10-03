// 通道连接引导弹窗：展示二维码或手动凭据输入表单。
import { useState } from 'react'
import { AlertTriangle, ExternalLink, Plus, RefreshCw, ShieldCheck, X } from 'lucide-react'
import { AppCardHeader, AppNotice } from '@/components/ui/app-primitives'
import { useI18n } from '@/app/i18n/use-i18n'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { apiJson } from '@/lib/http/api'
import {
  expiresIn,
  isManualPlatform,
  onboardingStatusLabel,
  providerCapability,
  providerName,
} from '@/features/channels/model/channel-providers'
import type { ManualCredentials, OnboardingJob } from '@/features/channels/model/channel-types'
import type { Notify } from '@/app/routes/route-context'

function errorMessage(caught: unknown) {
  return caught instanceof Error ? caught.message : String(caught)
}

export function OnboardingModal({
  job,
  onClose,
  onRetry,
  onSubmitCredentials,
  notify,
}: {
  job: OnboardingJob
  onClose: () => void | Promise<void>
  onRetry: () => void | Promise<void>
  onSubmitCredentials: (credentials: ManualCredentials) => void | Promise<void>
  notify: Notify
}) {
  const { t, language } = useI18n()
  const [code, setCode] = useState('')
  const [appId, setAppId] = useState('')
  const [appSecret, setAppSecret] = useState('')
  const [token, setToken] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const terminal = ['completed', 'failed', 'cancelled'].includes(job.status)
  const manual = job.manual || (isManualPlatform(job.platform) && job.mode !== 'qr')
  const credentialsValid =
    job.platform === 'qq'
      ? Boolean(appId.trim() && (appSecret.trim() || token.trim()))
      : Boolean(token.trim())
  const submitCredentials = async () => {
    if (!credentialsValid) return
    setSubmitting(true)
    try {
      await onSubmitCredentials({
        ...(appId.trim() ? { appId: appId.trim() } : {}),
        ...(appSecret.trim() ? { appSecret: appSecret.trim() } : {}),
        ...(token.trim() ? { token: token.trim() } : {}),
      })
    } finally {
      setSubmitting(false)
    }
  }
  const submitCode = async () => {
    try {
      await apiJson(
        `/api/channels/${job.platform}/onboarding/${encodeURIComponent(job.id || '')}/verify`,
        { method: 'POST', body: JSON.stringify({ code }) },
      )
      notify(t('channels:channelsPage.pairingCodeSubmitted'))
    } catch (caught) {
      notify(errorMessage(caught), 'error')
    }
  }
  return (
    <div
      className="modal-backdrop max-[650px]:p-[8px] fixed z-[70] inset-0 grid place-items-center overflow-y-auto bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)] [padding:20px] [overscroll-behavior:contain] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)] feishu-onboard-modal w-[min(470px,100%)]">
        <AppCardHeader>
          <div>
            <h2>
              {manual
                ? t('channels:channelsPage.configureName', { name: providerName(job.platform, t) })
                : t('channels:channelsPage.connectNameByQRCode', {
                    name: providerName(job.platform, t),
                  })}
            </h2>
            <p>
              {job.platform === 'feishu'
                ? t(
                    'channels:channelsPage.createTheBotAppThroughTheOfficialFeishuAuthorizationPage',
                  )
                : job.platform === 'weixin'
                  ? t('channels:channelsPage.signInToPersonalWeChatThroughTencentILinkBot')
                  : job.platform === 'qq'
                    ? t('channels:channelsPage.scanWithQQToCreateOfficialBot')
                    : t('channels:channelsPage.enterBotCredentialsForName', {
                        name: providerName(job.platform, t),
                      })}
            </p>
          </div>
          <Button
            variant="ghost"
            size="icon"
            aria-label={t('channels:channelsPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </Button>
        </AppCardHeader>
        {manual && !job.id && !terminal && (
          <div className="grid gap-[9px] [margin-top:14px]">
            {job.platform === 'qq' && (
              <>
                <FieldLabel variant="control">
                  {t('channels:channelsPage.qqAppId')}
                  <Input value={appId} onChange={(event) => setAppId(event.target.value)} />
                </FieldLabel>
                <FieldLabel variant="control">
                  {t('channels:channelsPage.qqAppSecret')}
                  <Input
                    type="password"
                    value={appSecret}
                    onChange={(event) => setAppSecret(event.target.value)}
                  />
                </FieldLabel>
                <small className="text-[var(--text-muted)]">
                  {t('channels:channelsPage.qqCredentialsOrToken')}
                </small>
              </>
            )}
            <FieldLabel variant="control">
              {t('channels:channelsPage.botToken')}
              <Input
                type="password"
                value={token}
                onChange={(event) => setToken(event.target.value)}
              />
            </FieldLabel>
            {job.setupUrl && (
              <AppNotice>
                <span>
                  <a href={job.setupUrl} target="_blank" rel="noreferrer">
                    {t('channels:channelsPage.openOfficialSetup')}
                    <ExternalLink size={12} />
                  </a>
                </span>
              </AppNotice>
            )}
            <Button
              size="lg"
              disabled={!credentialsValid || submitting}
              onClick={() => void submitCredentials()}
            >
              {submitting ? <RefreshCw className="animate-spin" size={14} /> : <Plus size={14} />}
              {t('channels:channelsPage.connect')}
            </Button>
          </div>
        )}
        {(!manual || job.id || terminal) && (
          <div
            className={`feishu-qr-stage [&_img]:w-[248px] [&_img]:max-w-[86%] [&_img]:[border:1px_solid_var(--stroke-soft)] [&_img]:rounded-[var(--r-md)] [&_img]:bg-[var(--lightbox-action-bg)] [&_img]:p-[8px] [&_img]:shadow-[0_12px_30px_-24px_var(--ink-strong)] [&_strong]:text-[13px] [&_p]:max-w-[330px] [&_p]:text-[var(--danger)] [&_p]:text-[12px] [&_p]:leading-[1.5] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&.completed]:text-[var(--success)] [&.failed]:text-[var(--danger)] flex min-h-[min(330px,52dvh)] flex-col items-center justify-center gap-[10px] [margin-top:14px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-md)] bg-[linear-gradient(180deg,var(--surface-highlight),var(--surface-subtle))] [padding:18px] text-center ${job.status}`}
          >
            {job.qrDataUrl ? (
              <img
                src={job.qrDataUrl}
                alt={t('channels:channelsPage.nameConnectionQRCode', {
                  name: providerName(job.platform, t),
                })}
              />
            ) : job.status === 'failed' ? (
              <AlertTriangle size={42} />
            ) : (
              <RefreshCw className="animate-spin" size={32} />
            )}
            <strong>
              {manual && !job.id
                ? t('channels:channelsPage.enterBotCredentialsForName', {
                    name: providerName(job.platform, t),
                  })
                : onboardingStatusLabel(job.status, t)}
            </strong>
            {job.error && <p>{job.error}</p>}
            {job.expireAt && !terminal && (
              <small>
                {t('channels:channelsPage.qrCodeExpiresTime', {
                  time: expiresIn(job.expireAt, language),
                })}
              </small>
            )}
          </div>
        )}
        {job.needsVerifyCode && (
          <div className="weixin-verify-code [&_input]:min-w-0 [&_input]:h-[34px] [&_input]:[border:1px_solid_var(--stroke)] [&_input]:rounded-[var(--r-sm)] [&_input]:bg-[var(--solid)] [&_input]:p-[0_10px] [&_input]:text-[var(--text)] [&_input]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_input]:text-[13px] [&_input]:tracking-[.08em] grid grid-cols-[minmax(0,1fr)_auto] gap-[7px] [margin-top:9px]">
            <input
              value={code}
              onChange={(event) => setCode(event.target.value.replace(/\D/g, '').slice(0, 8))}
              placeholder={t('channels:channelsPage.enterTheNumberShownOnYourPhone')}
            />
            <Button size="lg" disabled={!code} onClick={submitCode}>
              {t('channels:channelsPage.submitPairingCode')}
            </Button>
          </div>
        )}
        {job.qrUrl && !terminal && (
          <Button
            asChild
            variant="outline"
            size="lg"
            className="[margin-top:9px] no-underline w-full bg-surface-subtle"
          >
            <a href={job.qrUrl} target="_blank" rel="noreferrer">
              <ExternalLink size={14} />
              {t('channels:channelsPage.cannotScanOpenTheSignInLink')}
            </a>
          </Button>
        )}
        <AppNotice>
          <ShieldCheck size={15} />
          <span>
            <strong>{t('channels:channelsPage.persistentTwoWayConnection')}</strong>
            <small>
              {job.platform === 'feishu'
                ? t('channels:channelsPage.webSocketReceivesDirectMessagesAndGroupMentions')
                : job.platform === 'weixin'
                  ? t(
                      'channels:channelsPage.tencentILinkContinuouslyPollsDirectMessagesAndSupportsTextAndMediaReplies',
                    )
                  : providerCapability(job.platform, t)}
            </small>
          </span>
        </AppNotice>
        <div className="flex justify-end gap-[8px] [margin-top:18px]">
          <Button variant="outline" size="lg" className="bg-surface-subtle" onClick={onClose}>
            {terminal ? t('channels:channelsPage.off') : t('channels:channelsPage.cancel')}
          </Button>
          {job.status === 'failed' && (
            <Button size="lg" onClick={onRetry}>
              <RefreshCw size={14} />
              {manual
                ? t('channels:channelsPage.retry')
                : t('channels:channelsPage.generateANewQRCode')}
            </Button>
          )}
        </div>
      </section>
    </div>
  )
}
