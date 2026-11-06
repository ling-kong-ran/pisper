// Computer Use：指令发给桌面前端，由原生层截屏和操作键鼠。
// 同一时间只有一个会话能操作（屏幕只有一块），由原生层判定并报 busy。
import { DesktopHostChannel } from './desktop-host-channel.mjs'

// 打字最多 5000 字，分段发送时可能比普通动作慢得多。
const REQUEST_TIMEOUT_MS = 120_000
const MAX_IMAGE_BASE64_CHARS = 32_000_000

export class DesktopComputerService extends DesktopHostChannel {
  constructor(options = {}) {
    super({ timeoutMs: REQUEST_TIMEOUT_MS, ...options })
  }

  async execute(sessionId, input, { signal } = {}) {
    if (!this.host)
      throw new Error(
        'Computer use needs the Pisper desktop app to be open. It is not available from the web version or on phones.',
      )
    const value = await this.request(sessionId, input, signal)
    const record = value && typeof value === 'object' ? value : {}
    if (input.action === 'finish') return { action: 'finish' }
    const image = typeof record.imageBase64 === 'string' ? record.imageBase64 : ''
    if (!image || image.length > MAX_IMAGE_BASE64_CHARS)
      throw new Error('The desktop app did not return a usable screenshot.')
    return {
      action: input.action,
      imageBase64: image,
      width: Number(record.width) || 0,
      height: Number(record.height) || 0,
    }
  }

  // 收起遮罩、还原主窗口。这一轮结束或要用户回答时调用：主窗口最小化着，用户看不到要回答什么。
  release(sessionId) {
    this.host?.send('release', { sessionId })
  }
}
