// Bot 的电脑（B6）的展示逻辑（纯函数）：电脑名称、在那台电脑上要运行的配对命令。
import type { Translate } from '@/app/navigation'
import type { BotEnvironment, BotRemoteInfo } from './bots-api'

export function environmentLabel(t: Translate, environment: BotEnvironment | undefined) {
  if (!environment || environment.kind === 'local') return t('bots:computer.local')
  return environment.name || t('bots:computer.unnamed')
}

// 节点要运行的两条命令：远程访问的第一个地址与证书指纹都写进去，目录由用户改成自己的。
// 远程访问没打开时没有地址，用占位文字提示。
export function executorCommands(
  pairing: { code: string; remote: BotRemoteInfo },
  placeholders: { brain: string; fingerprint: string; root: string },
) {
  const brain = pairing.remote.endpoints[0] || placeholders.brain
  const fingerprint = pairing.remote.fingerprint || placeholders.fingerprint
  return [
    `npx pisper executor pair --brain ${brain} --fingerprint ${fingerprint} --code ${pairing.code} --root ${placeholders.root}`,
    'npx pisper executor',
  ]
}
