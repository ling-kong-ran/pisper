// 远程机器路由：列出、添加和修改 SSH 机器、测试连接、移除；让对方连过来的配对码也在这里。
// 只做校验和调用服务，经能力 remoteMachines 把关，不受 Bots 实验开关影响。
// 密码只进不出：请求里可以带 password，任何响应里都只有 passwordSet。主机公钥是公开信息，可以来回传。
import { remoteInfo } from './executors.mjs'
import { errorPayload } from '../../../shared/errors.mjs'
import { respondError } from './bots.mjs'
import { isSshMachineId } from '../../services/ssh-machine-service.mjs'

function machineRoute(handler) {
  return async (context) => {
    try {
      await handler(context)
    } catch (error) {
      respondError(context.json, error)
    }
  }
}

export const machineRoutes = [
  {
    method: 'GET',
    path: '/api/machines',
    handler: machineRoute(({ runtime, services, json }) => {
      json(200, { machines: runtime.listMachines(), remote: remoteInfo(services) })
    }),
  },
  {
    method: 'POST',
    path: '/api/machines/ssh',
    handler: machineRoute(async ({ runtime, body, json }) => {
      json(201, await runtime.sshMachines.add(await body()))
    }),
  },
  {
    // 保存前先测：连不上不是接口错误，结果里带分类，界面据此给出下一步。
    method: 'POST',
    path: '/api/machines/ssh/test',
    handler: machineRoute(async ({ runtime, body, json }) => {
      const input = await body()
      try {
        json(200, { ok: true, ...(await runtime.sshMachines.test(input)) })
      } catch (error) {
        if (
          error?.code === 'invalid_target' ||
          error?.code === 'invalid_port' ||
          error?.code === 'invalid'
        )
          throw error
        json(200, {
          ok: false,
          ...errorPayload(error),
          code: String(error?.code || 'connection_failed'),
          message: String(error?.message || error),
        })
      }
    }),
  },
  {
    // 密码登录前取对方的主机公钥给用户核对指纹：只握手，不带任何凭据。连不上同样不是接口错误。
    method: 'POST',
    path: '/api/machines/ssh/host-key',
    handler: machineRoute(async ({ runtime, body, json }) => {
      const input = await body()
      try {
        json(200, { ok: true, hostKey: await runtime.sshMachines.hostKey(input) })
      } catch (error) {
        if (error?.code === 'invalid_target' || error?.code === 'invalid_port') throw error
        json(200, {
          ok: false,
          ...errorPayload(error),
          code: String(error?.code || 'connection_failed'),
          message: String(error?.message || error),
        })
      }
    }),
  },
  {
    method: 'POST',
    path: '/api/machines/pairing',
    handler: machineRoute(({ runtime, services, json }) => {
      json(200, { pairing: runtime.createEnvironmentPairing(), remote: remoteInfo(services) })
    }),
  },
  {
    method: 'POST',
    path: '/api/machines/:machineId/check',
    handler: machineRoute(async ({ runtime, params, json }) => {
      json(200, await runtime.sshMachines.check(params.machineId))
    }),
  },
  {
    method: 'POST',
    path: '/api/machines/:machineId/folder',
    handler: machineRoute(async ({ runtime, params, body, json }) => {
      const input = await body()
      json(200, {
        cwd: await runtime.resolveMachineFolder(
          params.machineId,
          String(input?.cwd ?? '').slice(0, 4096),
        ),
      })
    }),
  },
  {
    method: 'DELETE',
    path: '/api/machines/:machineId',
    handler: machineRoute(async ({ runtime, params, json }) => {
      const removed = await runtime.removeMachine(params.machineId)
      if (!removed) json(404, { error: 'Machine not found.', code: 'machine_not_found' })
      else json(200, { ok: true })
    }),
  },
  {
    // 只有 SSH 机器能改；执行节点的设定在对方那台电脑上。
    method: 'PATCH',
    path: '/api/machines/:machineId',
    handler: machineRoute(async ({ runtime, params, body, json }) => {
      if (!isSshMachineId(params.machineId) || !runtime.sshMachines.get(params.machineId)) {
        json(404, { error: 'Machine not found.', code: 'machine_not_found' })
        return
      }
      json(200, await runtime.sshMachines.update(params.machineId, await body()))
    }),
  },
]
