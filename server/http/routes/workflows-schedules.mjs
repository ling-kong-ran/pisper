export const workflowScheduleRoutes = [
  {
    method: 'GET',
    path: '/api/schedules',
    async handler({ runtime, json }) {
      json(200, await runtime.getSchedules())
    },
  },
  {
    method: 'POST',
    path: '/api/schedules',
    async handler({ runtime, body, json }) {
      json(201, await runtime.createSchedule(await body()))
    },
  },
  {
    method: 'POST',
    path: '/api/schedules/:scheduleId/run',
    async handler({ runtime, params, json }) {
      const result = await runtime.runSchedule(params.scheduleId)
      if (!result) json(404, { error: '定时任务不存在。' })
      else json(202, result)
    },
  },
  {
    method: 'PATCH',
    path: '/api/schedules/:scheduleId',
    async handler({ runtime, params, body, json }) {
      const result = await runtime.updateSchedule(params.scheduleId, await body())
      if (!result) json(404, { error: '定时任务不存在。' })
      else json(200, result)
    },
  },
  {
    method: 'DELETE',
    path: '/api/schedules/:scheduleId',
    async handler({ runtime, params, json }) {
      const deleted = await runtime.deleteSchedule(params.scheduleId)
      if (!deleted) json(404, { error: '定时任务不存在。' })
      else json(200, { deleted: true })
    },
  },
  {
    method: 'GET',
    path: '/api/workflows',
    async handler({ runtime, json }) {
      json(200, await runtime.getWorkflows())
    },
  },
  {
    method: 'POST',
    path: '/api/workflows',
    async handler({ runtime, body, json }) {
      json(201, await runtime.createWorkflow(await body()))
    },
  },
  {
    method: 'POST',
    path: '/api/workflows/runs/:runId/stop',
    async handler({ runtime, params, json }) {
      const result = await runtime.stopWorkflowRun(params.runId)
      if (!result) json(404, { error: '工作流运行不存在或已经结束。' })
      else json(202, result)
    },
  },
  {
    method: 'POST',
    path: '/api/workflows/:workflowId/run',
    async handler({ runtime, params, json }) {
      const result = await runtime.runWorkflow(params.workflowId)
      if (!result) json(404, { error: '工作流不存在。' })
      else json(202, result)
    },
  },
  {
    method: 'PATCH',
    path: '/api/workflows/:workflowId',
    async handler({ runtime, params, body, json }) {
      const result = await runtime.updateWorkflow(params.workflowId, await body())
      if (!result) json(404, { error: '工作流不存在。' })
      else json(200, result)
    },
  },
  {
    method: 'DELETE',
    path: '/api/workflows/:workflowId',
    async handler({ runtime, params, json }) {
      const deleted = await runtime.deleteWorkflow(params.workflowId)
      if (!deleted) json(404, { error: '工作流不存在。' })
      else json(200, { deleted: true })
    },
  },
]
