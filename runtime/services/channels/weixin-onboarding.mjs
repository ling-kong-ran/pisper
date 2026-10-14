// 微信接入引导：扫码 + 验证码 + 设备绑定（pad/bot）的多阶段登录流程。
import { randomUUID } from 'node:crypto'
import QRCode from 'qrcode'
import { WeixinProtocol, WEIXIN_API_BASE, WEIXIN_CDN_BASE } from './weixin-protocol.mjs'

function publicJob(job) {
  return {
    id: job.id,
    platform: 'weixin',
    status: job.status,
    qrUrl: job.qrUrl || '',
    qrDataUrl: job.qrDataUrl || '',
    expireAt: job.expireAt || null,
    error: job.error || '',
    needsVerifyCode: job.status === 'verification_required',
  }
}

function delay(ms, signal) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(resolve, ms)
    signal?.addEventListener(
      'abort',
      () => {
        clearTimeout(timer)
        reject(new Error('abort'))
      },
      { once: true },
    )
  })
}

export class WeixinOnboardingService {
  constructor({
    onCompleted,
    protocol = new WeixinProtocol(),
    renderQr = (url) => QRCode.toDataURL(url, { width: 248, margin: 2, errorCorrectionLevel: 'M' }),
  }) {
    this.onCompleted = onCompleted
    this.protocol = protocol
    this.renderQr = renderQr
    this.jobs = new Map()
  }

  async start({ localTokens = [] } = {}) {
    for (const job of this.jobs.values())
      if (!['completed', 'failed', 'cancelled'].includes(job.status)) job.controller.abort()
    const response = await this.protocol.startQr({ localTokens })
    if (!response.qrcode || !response.qrcode_img_content)
      throw new Error('微信登录服务未返回二维码。')
    const job = {
      id: randomUUID(),
      controller: new AbortController(),
      status: 'waiting',
      qrcode: response.qrcode,
      qrUrl: response.qrcode_img_content,
      qrDataUrl: await this.renderQr(response.qrcode_img_content),
      expireAt: new Date(Date.now() + 5 * 60_000).toISOString(),
      error: '',
      baseUrl: WEIXIN_API_BASE,
      verifyCode: '',
      verifyVersion: 0,
    }
    this.jobs.set(job.id, job)
    job.promise = this.poll(job).catch((error) => {
      if (job.controller.signal.aborted) job.status = 'cancelled'
      else {
        job.status = 'failed'
        job.error = error instanceof Error ? error.message : String(error)
      }
    })
    return publicJob(job)
  }

  async poll(job) {
    let refreshes = 0
    while (!job.controller.signal.aborted && Date.now() < new Date(job.expireAt).getTime()) {
      const result = await this.protocol.pollQr({
        qrcode: job.qrcode,
        baseUrl: job.baseUrl,
        verifyCode: job.verifyCode,
        signal: job.controller.signal,
      })
      if (result.status === 'wait') job.status = 'waiting'
      else if (result.status === 'scaned') {
        job.status = 'scanned'
        job.verifyCode = ''
      } else if (result.status === 'need_verifycode') {
        job.status = 'verification_required'
        const version = job.verifyVersion
        while (!job.controller.signal.aborted && version === job.verifyVersion)
          await delay(500, job.controller.signal)
        continue
      } else if (result.status === 'verify_code_blocked')
        throw new Error('配对码多次输入错误，请重新扫码。')
      else if (result.status === 'scaned_but_redirect') {
        if (result.redirect_host) job.baseUrl = `https://${result.redirect_host}`
        job.status = 'scanned'
      } else if (result.status === 'binded_redirect')
        throw new Error('该微信已绑定当前机器人，请先解除旧连接后重试。')
      else if (result.status === 'expired') {
        refreshes += 1
        if (refreshes > 2) throw new Error('微信二维码已多次过期，请重新开始。')
        const next = await this.protocol.startQr()
        job.qrcode = next.qrcode
        job.qrUrl = next.qrcode_img_content
        job.qrDataUrl = await this.renderQr(job.qrUrl)
        job.expireAt = new Date(Date.now() + 5 * 60_000).toISOString()
        job.status = 'waiting'
      } else if (result.status === 'confirmed') {
        if (!result.bot_token || !result.ilink_bot_id)
          throw new Error('微信确认成功，但登录凭据不完整。')
        job.status = 'connecting'
        await this.onCompleted({
          accountId: result.ilink_bot_id,
          token: result.bot_token,
          ownerUserId: result.ilink_user_id || '',
          baseUrl: result.baseurl || job.baseUrl || WEIXIN_API_BASE,
          cdnBaseUrl: WEIXIN_CDN_BASE,
        })
        job.status = 'completed'
        return
      }
      await delay(800, job.controller.signal)
    }
    if (!job.controller.signal.aborted) throw new Error('微信扫码登录已超时。')
  }

  verify(id, code) {
    const job = this.jobs.get(id)
    if (!job) return null
    const value = String(code || '').trim()
    if (!/^\d{4,8}$/.test(value)) throw new Error('请输入微信显示的数字配对码。')
    job.verifyCode = value
    job.verifyVersion += 1
    job.status = 'scanned'
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
