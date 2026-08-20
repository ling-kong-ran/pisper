// mDNS 广播：远程模式开启时向局域网广播 _pisper._tcp 服务，
// 让移动端无需扫码也能发现桌面端。广播失败（无组播权限、容器网络等）
// 只记录警告，不影响远程访问主流程（二维码/手动输入仍可用）。
export class MdnsAdvertiser {
  constructor() {
    this.responder = null
    this.service = null
    this.error = null
  }

  async start({ name, port, txt } = {}) {
    await this.stop()
    try {
      // ciao 是 CommonJS 包：ESM 默认导入拿到 module.exports 再解构，兼容两种导出形态。
      const mod = await import('@homebridge/ciao')
      const ciao = mod.default || mod
      const Responder = ciao.Responder
      this.responder = new Responder()
      this.service = this.responder.createService({
        name: name || 'Pisper',
        type: 'pisper',
        port,
        txt: txt || {},
      })
      await this.service.advertise()
      this.error = null
      return true
    } catch (error) {
      this.error = error instanceof Error ? error.message : String(error)
      this.service = null
      return false
    }
  }

  async stop() {
    if (this.service) {
      try {
        await this.service.end()
      } catch {
        // 停止广播失败不影响关闭流程。
      }
      this.service = null
    }
    if (this.responder) {
      try {
        await this.responder.shutdown()
      } catch {
        // 同上。
      }
      this.responder = null
    }
  }

  status() {
    return { advertising: Boolean(this.service), error: this.error }
  }
}
