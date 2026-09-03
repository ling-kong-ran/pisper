// QQ 官方机器人扫码绑定：调用腾讯 QQ Bot Connector 获取一次性二维码和官方凭据。
import { randomUUID } from 'node:crypto'
import QRCode from 'qrcode'

const defaultStartQrConnect = async (callbacks, options) => {
  const { startQrConnect } = await import('@tencent-connect/qqbot-connector')
  return startQrConnect(callbacks, options)
}

function publicJob(job) {
  return {
    id: job.id,
    platform: 'qq',
    mode: 'qr',
    status: job.status,
    qrUrl: job.qrUrl || '',
    qrDataUrl: job.qrDataUrl || '',
    expireAt: job.expireAt || null,
    error: job.error || '',
  }
}

export class QQOnboardingService {
  constructor({
    onCompleted,
    startQrConnectImpl = defaultStartQrConnect,
    renderQr = (url) => QRCode.toDataURL(url, { width: 248, margin: 2, errorCorrectionLevel: 'M' }),
  }) {
    this.onCompleted = onCompleted
    this.startQrConnectImpl = startQrConnectImpl
    this.renderQr = renderQr
    this.jobs = new Map()
  }

  async start() {
    for (const job of this.jobs.values()) {
      if (!['completed', 'failed', 'cancelled'].includes(job.status)) {
        job.controller.abort()
        job.stop?.()
      }
    }
    const job = {
      id: randomUUID(),
      controller: new AbortController(),
      status: 'starting',
      qrUrl: '',
      qrDataUrl: '',
      expireAt: null,
      error: '',
      stop: null,
    }
    this.jobs.set(job.id, job)

    let readyResolve
    let readyReject
    const ready = new Promise((resolve, reject) => {
      readyResolve = resolve
      readyReject = reject
    })
    let readySettled = false
    const resolveReady = (value) => {
      if (readySettled) return
      readySettled = true
      readyResolve(value)
    }
    const rejectReady = (error) => {
      if (readySettled) return
      readySettled = true
      readyReject(error)
    }

    const callbacks = {
      onQrDisplayed: (url) => {
        job.qrUrl = String(url || '')
        job.expireAt = new Date(Date.now() + 5 * 60_000).toISOString()
        job.status = 'waiting'
        Promise.resolve(this.renderQr(job.qrUrl))
          .then((dataUrl) => {
            job.qrDataUrl = dataUrl
            resolveReady(publicJob(job))
          })
          .catch(rejectReady)
      },
      onQrExpired: () => {
        job.status = 'waiting'
      },
      onSuccess: (credentials) => {
        void (async () => {
          const credential = credentials?.[0]
          if (!credential?.appId || !credential?.appSecret)
            throw new Error('QQ 扫码成功，但未返回完整的官方机器人凭据。')
          job.status = 'connecting'
          await this.onCompleted({
            appId: credential.appId,
            appSecret: credential.appSecret,
            ownerUserId: credential.userOpenid || '',
          })
          job.status = 'completed'
        })().catch((error) => {
          job.status = 'failed'
          job.error = error instanceof Error ? error.message : String(error)
        })
      },
      onFailure: (error) => {
        if (job.controller.signal.aborted) job.status = 'cancelled'
        else {
          job.status = 'failed'
          job.error = error instanceof Error ? error.message : String(error)
        }
        rejectReady(error instanceof Error ? error : new Error(String(error)))
      },
    }

    job.promise = Promise.resolve(
      this.startQrConnectImpl(callbacks, {
        displayQrCodeToConsole: false,
        source: 'pisper',
        signal: job.controller.signal,
      }),
    )
      .then((stop) => {
        job.stop = typeof stop === 'function' ? stop : null
      })
      .catch((error) => {
        callbacks.onFailure(error)
      })

    let timeoutId
    const timeout = new Promise((_, reject) => {
      timeoutId = setTimeout(() => reject(new Error('QQ 扫码地址生成超时。')), 15_000)
    })
    try {
      await Promise.race([ready, timeout])
    } finally {
      clearTimeout(timeoutId)
    }
    return publicJob(job)
  }

  get(id) {
    const job = this.jobs.get(id)
    return job ? publicJob(job) : null
  }

  cancel(id) {
    const job = this.jobs.get(id)
    if (!job) return false
    job.controller.abort()
    job.stop?.()
    job.status = 'cancelled'
    return true
  }

  dispose() {
    for (const job of this.jobs.values()) {
      job.controller.abort()
      job.stop?.()
    }
    this.jobs.clear()
  }
}
