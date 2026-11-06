// 启动结果的唯一出口：Runtime 起不来时说明原因（否则用户只看到一个什么都加载不出来的窗口）；
// 有数据文件被挪开或某个服务没起来时提示一次，让用户知道少了什么、原文件还在。
import { useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { PageState } from '@/components/app/PageState'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { fetchRuntimeStartupStatus, type RuntimeStartupStatus } from './runtime-startup-api'

const POLL_INTERVAL_MS = 1_000
// 首次升级补索引可能要几分钟；再久就不问了，不留一个永远在跑的定时器。
const POLL_LIMIT_MS = 10 * 60_000

export function RuntimeStartupGate() {
  const { t } = useI18n()
  const [status, setStatus] = useState<RuntimeStartupStatus | null>(null)
  const [dismissed, setDismissed] = useState(false)

  useEffect(() => {
    const controller = new AbortController()
    const deadline = Date.now() + POLL_LIMIT_MS
    let timer: number | undefined
    const poll = async () => {
      let done = false
      try {
        const next = await fetchRuntimeStartupStatus(controller.signal)
        if (controller.signal.aborted) return
        if (next) setStatus(next)
        done = !next || next.state !== 'pending'
      } catch {
        // Runtime 还在准备或连接抖动：下一轮再问。
      }
      if (!done && !controller.signal.aborted && Date.now() < deadline)
        timer = window.setTimeout(() => void poll(), POLL_INTERVAL_MS)
    }
    void poll()
    return () => {
      controller.abort()
      window.clearTimeout(timer)
    }
  }, [])

  if (status?.state === 'failed') {
    const failure = status.failure
    return (
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-background">
        <PageState
          kind="error"
          title={t('common:runtimeStartup.failedTitle')}
          description={
            <>
              {t('common:runtimeStartup.failedHint')}
              {failure && (
                <code className="mt-2 block font-mono select-text">
                  {failure.message.startsWith(failure.code)
                    ? failure.message
                    : `${failure.code}: ${failure.message}`}
                </code>
              )}
            </>
          }
        />
      </div>
    )
  }

  if (dismissed || !status) return null
  const { setAsideFiles, failedServices } = status
  if (!setAsideFiles.length && !failedServices.length) return null
  return (
    <Alert
      className="fixed right-4 bottom-[calc(1rem+var(--pisper-safe-area-bottom,0px))] z-50 w-[min(26rem,calc(100vw-2rem))] shadow-lg"
      data-runtime-startup-notice
    >
      <AlertTitle>{t('common:runtimeStartup.noticeTitle')}</AlertTitle>
      <AlertDescription className="grid gap-1">
        {setAsideFiles.length > 0 && (
          <p>{t('common:runtimeStartup.setAside', { files: setAsideFiles.join('、') })}</p>
        )}
        {failedServices.length > 0 && (
          <p>
            {t('common:runtimeStartup.failedServices', { services: failedServices.join('、') })}
          </p>
        )}
        <Button
          type="button"
          variant="outline"
          className="mt-1 justify-self-end"
          onClick={() => setDismissed(true)}
        >
          {t('common:runtimeStartup.dismiss')}
        </Button>
      </AlertDescription>
    </Alert>
  )
}
