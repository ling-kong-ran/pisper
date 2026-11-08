// Runtime 的结构化错误码 → 当前语言的文案和下一步。不认识的错误码用 Runtime 给的原文。
import type { Translate } from '@/app/navigation'

export function machineFailureMessage(t: Translate, code: string, fallback = '') {
  switch (code) {
    case 'connection_refused':
      return t('machines:failure.refused')
    case 'unreachable':
      return t('machines:failure.unreachable')
    case 'auth_failed':
      return t('machines:failure.auth')
    case 'host_key_changed':
      return t('machines:failure.hostKey')
    case 'ssh_client_missing':
      return t('machines:failure.clientMissing')
    case 'ssh_timeout':
      return t('machines:failure.timeout')
    case 'invalid_target':
      return t('machines:failure.invalidTarget')
    case 'invalid_port':
      return t('machines:failure.invalidPort')
    case 'machine_folder_missing':
      return t('machines:failure.folderMissing')
    case 'machine_folder_outside_roots':
      return t('machines:failure.outsideRoots')
    case 'machine_offline':
      return t('machines:failure.offline')
    case 'machine_not_found':
      return t('machines:failure.notFound')
    case 'too_many_machines':
      return t('machines:failure.tooMany')
    default:
      return fallback || t('machines:failure.generic')
  }
}

// 让对方连过来要运行的两条命令：远程访问的地址和证书指纹都写进去，目录由用户替换。
// 远程访问没打开时没有地址，用占位文字提示。
export function executorCommands(
  pairing: { code: string; remote: { endpoints: string[]; fingerprint: string | null } },
  placeholders: { brain: string; fingerprint: string; root: string },
) {
  const brain = pairing.remote.endpoints[0] || placeholders.brain
  const fingerprint = pairing.remote.fingerprint || placeholders.fingerprint
  return [
    `npx pisper executor pair --brain ${brain} --fingerprint ${fingerprint} --code ${pairing.code} --root ${placeholders.root}`,
    'npx pisper executor',
  ]
}
