// 飞书接入引导（onboarding）：引导用户注册应用、扫码授权，分阶段推进状态机，
// 验证码/二维码均通过状态轮询查询。
import { randomUUID } from 'node:crypto'
import QRCode from 'qrcode'

// Lazily loaded: @larksuiteoapi/node-sdk is large and only needed during Feishu onboarding.
const defaultRegisterApp = async (options) => {
  const { registerApp } = await import('@larksuiteoapi/node-sdk')
  return registerApp(options)
}

const SCOPES = [
  'im:message:send_as_bot',
  'im:message.p2p_msg:readonly',
  'im:message.group_at_msg:readonly',
  'im:resource',
]

function publicJob(job) {
  return {
    id: job.id,
    status: job.status,
    qrUrl: job.qrUrl || '',
    qrDataUrl: job.qrDataUrl || '',
    userCode: job.userCode || '',
    expireAt: job.expireAt || null,
    error: job.error || '',
  }
}

export class FeishuOnboardingService {
  constructor({
    onCompleted,
    registerAppImpl = defaultRegisterApp,
    renderQr = (url) => QRCode.toDataURL(url, { width: 248, margin: 2, errorCorrectionLevel: 'M' }),
  }) {
    this.onCompleted = onCompleted
    this.registerAppImpl = registerAppImpl
    this.renderQr = renderQr
    this.jobs = new Map()
  }

  async start() {
    for (const job of this.jobs.values()) {
      if (['starting', 'waiting', 'authorizing', 'connecting'].includes(job.status))
        job.controller.abort()
    }
    const id = randomUUID()
    const controller = new AbortController()
    const job = {
      id,
      controller,
      status: 'starting',
      qrUrl: '',
      qrDataUrl: '',
      expireAt: null,
      error: '',
    }
    this.jobs.set(id, job)
    let readyResolve
    let readyReject
    const ready = new Promise((resolve, reject) => {
      readyResolve = resolve
      readyReject = reject
    })

    job.promise = this.registerAppImpl({
      source: 'pisper',
      signal: controller.signal,
      createOnly: true,
      appPreset: {
        name: 'Pisper Agent',
        desc: '通过飞书与本机 Pisper Agent 进行双向对话',
      },
      addons: {
        preset: false,
        scopes: { tenant: SCOPES },
        events: { items: { tenant: ['im.message.receive_v1'] } },
      },
      onQRCodeReady: (info) => {
        job.qrUrl = info.url
        job.expireAt = new Date(Date.now() + Number(info.expireIn || 600) * 1000).toISOString()
        job.status = 'waiting'
        Promise.resolve(this.renderQr(info.url))
          .then((dataUrl) => {
            job.qrDataUrl = dataUrl
            readyResolve(publicJob(job))
          })
          .catch(readyReject)
      },
      onStatusChange: ({ status }) => {
        if (status === 'domain_switched') job.status = 'authorizing'
        else if (job.status === 'starting') job.status = 'waiting'
      },
    })
      .then(async (result) => {
        job.status = 'connecting'
        await this.onCompleted({
          appId: result.client_id,
          appSecret: result.client_secret,
          ownerOpenId: result.user_info?.open_id || '',
          domain: result.user_info?.tenant_brand === 'lark' ? 'lark' : 'feishu',
        })
        job.status = 'completed'
        return result
      })
      .catch((error) => {
        if (error?.code === 'abort' || controller.signal.aborted) job.status = 'cancelled'
        else {
          job.status = 'failed'
          job.error = error?.description || error?.message || String(error)
        }
        readyReject(error)
      })

    let timeoutId
    const timeout = new Promise((_, reject) => {
      timeoutId = setTimeout(() => reject(new Error('飞书扫码地址生成超时。')), 15_000)
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
    job.status = 'cancelled'
    return true
  }

  dispose() {
    for (const job of this.jobs.values()) job.controller.abort()
    this.jobs.clear()
  }
}
