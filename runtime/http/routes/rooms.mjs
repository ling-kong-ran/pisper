// Bot 群聊路由：群的增删改查、消息（分页、轮询增量、发、撤回）、工作卡上的批准与停止、简报。
// 只做门控、调用运行时与序列化；规则在 runtime/runtime/room-methods.mjs 与 runtime/services/rooms/。
// 跟着 Bots：Runtime 能力 bots（经 CAPABILITY_ROUTES）与实验开关 bots，关着时一律 409。
import { botRoute } from './bots.mjs'

const id = (params) => String(params.roomId || '')

export const roomRoutes = [
  {
    method: 'GET',
    path: '/api/rooms',
    handler: botRoute(({ runtime, json }) => {
      json(200, { rooms: runtime.listRooms() })
    }),
  },
  {
    method: 'POST',
    path: '/api/rooms',
    handler: botRoute(async ({ runtime, body, json }) => {
      json(201, { room: await runtime.createRoom(await body()) })
    }),
  },
  {
    method: 'GET',
    path: '/api/rooms/:roomId',
    handler: botRoute(async ({ runtime, params, json }) => {
      json(200, await runtime.getRoom(id(params)))
    }),
  },
  {
    method: 'PATCH',
    path: '/api/rooms/:roomId',
    handler: botRoute(async ({ runtime, params, body, json }) => {
      json(200, { room: await runtime.updateRoom(id(params), await body()) })
    }),
  },
  {
    method: 'DELETE',
    path: '/api/rooms/:roomId',
    handler: botRoute(async ({ runtime, params, json }) => {
      json(200, { deleted: await runtime.deleteRoom(id(params)) })
    }),
  },
  // ?before=<seq> 往前翻；?after=<seq> 轮询增量（同时带回还在进行中的工作卡）。
  {
    method: 'GET',
    path: '/api/rooms/:roomId/messages',
    handler: botRoute(async ({ runtime, params, url, json }) => {
      const query = url?.searchParams
      json(
        200,
        await runtime.listRoomMessages(id(params), {
          before: query?.get('before') ?? undefined,
          after: query?.get('after') ?? undefined,
          watch: query?.get('watch') ?? undefined,
          limit: query?.get('limit') ?? undefined,
        }),
      )
    }),
  },
  {
    method: 'POST',
    path: '/api/rooms/:roomId/messages',
    handler: botRoute(async ({ runtime, params, body, json }) => {
      json(201, await runtime.postRoomMessage(id(params), await body()))
    }),
  },
  {
    method: 'POST',
    path: '/api/rooms/:roomId/messages/:messageId/recall',
    handler: botRoute(async ({ runtime, params, json }) => {
      json(200, await runtime.recallRoomMessage(id(params), params.messageId))
    }),
  },
  {
    method: 'POST',
    path: '/api/rooms/:roomId/messages/:messageId/approval',
    handler: botRoute(async ({ runtime, params, body, json }) => {
      const input = await body()
      json(
        200,
        await runtime.resolveRoomApproval(id(params), params.messageId, {
          approvalId: input?.approvalId,
          approve: input?.approve === true,
        }),
      )
    }),
  },
  {
    method: 'POST',
    path: '/api/rooms/:roomId/messages/:messageId/stop',
    handler: botRoute(async ({ runtime, params, json }) => {
      json(200, await runtime.stopRoomTurn(id(params), params.messageId))
    }),
  },
  // 简报：{ botId } 请那个 Bot 整理（不给就用群设定里维护简报的 Bot）；PUT 是你自己改。
  {
    method: 'POST',
    path: '/api/rooms/:roomId/brief',
    handler: botRoute(async ({ runtime, params, body, json }) => {
      const input = await body()
      json(200, { brief: await runtime.refreshRoomBrief(id(params), { botId: input?.botId }) })
    }),
  },
  {
    method: 'PUT',
    path: '/api/rooms/:roomId/brief',
    handler: botRoute(async ({ runtime, params, body, json }) => {
      json(200, { brief: await runtime.editRoomBrief(id(params), await body()) })
    }),
  },
]
