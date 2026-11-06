// 桌面前端要接的两条 Agent 指令通道：右侧浏览器和 Computer Use。
import { startAgentBrowserHost } from './agent-browser-host'
import { startAgentComputerHost } from './agent-computer-host'

type Translate = (key: string) => string

export function startDesktopHosts(t: Translate) {
  const stops = [
    startAgentBrowserHost(() => t('common:webPreview.agentOverlay')),
    startAgentComputerHost(() => ({
      title: t('common:computerUse.title'),
      // Windows 上是全屏遮罩，按 Esc 停止；macOS 和 Linux 是带停止按钮的小窗口。
      hint:
        window.pisperDesktop?.platform === 'win32'
          ? t('common:computerUse.hintEsc')
          : t('common:computerUse.hintButton'),
      stop: t('common:computerUse.stop'),
    })),
  ]
  return () => {
    for (const stop of stops) stop()
  }
}
