// 工作流与定时任务路由：工作流/定时任务的 CRUD 与立即运行、审批、停止。
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
    path: '/api/workflows/import',
    async handler({ runtime, body, json }) {
      json(201, await runtime.importWorkflow(await body()))
    },
  },
  {
    method: 'GET',
    path: '/api/workflow-runs/:runId',
    handler({ runtime, params, json }) {
      const run = runtime.getWorkflowRun(params.runId)
      if (!run) json(404, { error: '工作流运行不存在。' })
      else json(200, run)
    },
  },
  {
    method: 'POST',
    path: '/api/workflow-runs/:runId/retry',
    async handler({ runtime, params, json }) {
      const result = await runtime.retryWorkflowRun(params.runId)
      if (!result) json(404, { error: '工作流运行不存在或不能重试。' })
      else json(202, result)
    },
  },
  {
    method: 'POST',
    path: '/api/workflow-runs/:runId/approvals/:nodeId',
    async handler({ runtime, params, body, json }) {
      const result = await runtime.resolveWorkflowApproval(
        params.runId,
        params.nodeId,
        await body(),
      )
      if (!result) json(404, { error: '待审批节点不存在或已经处理。' })
      else json(200, result)
    },
  },
  {
    method: 'POST',
    path: '/api/workflow-runs/:runId/stop',
    async handler({ runtime, params, json }) {
      const result = await runtime.stopWorkflowRun(params.runId)
      if (!result) json(404, { error: '工作流运行不存在或已经结束。' })
      else json(202, result)
    },
  },
  {
    method: 'POST',
    path: '/api/workflows/:workflowId/run',
    async handler({ runtime, params, body, json }) {
      const result = await runtime.runWorkflow(params.workflowId, await body())
      if (!result) json(404, { error: '工作流不存在。' })
      else json(202, result)
    },
  },
  {
    method: 'POST',
    path: '/api/workflows/:workflowId/duplicate',
    async handler({ runtime, params, body, json }) {
      const result = await runtime.duplicateWorkflow(params.workflowId, await body())
      if (!result) json(404, { error: '工作流不存在。' })
      else json(201, result)
    },
  },
  {
    method: 'GET',
    path: '/api/workflows/:workflowId/export',
    handler({ runtime, params, json }) {
      const result = runtime.exportWorkflow(params.workflowId)
      if (!result) json(404, { error: '工作流不存在。' })
      else json(200, result)
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
