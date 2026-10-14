// 工作流与定时任务路由：工作流/定时任务的 CRUD 与立即运行、审批、停止。
import {
  exportPortableWorkflowBundle,
  importPortableWorkflowBundle,
} from '../../services/workflow-portable-bundle.mjs'
import { MAX_WORKFLOW_BUNDLE_BYTES } from '../../services/workflow-bundle-archive.mjs'
import { randomUUID } from 'node:crypto'
import {
  normalizeWorkflowImageSettings,
  workflowImageError,
} from '../../../shared/workflow-image-nodes.mjs'
import { parseWorkflowMedia } from '../../../shared/workflow-inputs.mjs'

export const workflowScheduleRoutes = [
  {
    method: 'POST',
    path: '/api/workflow-image-process',
    async handler({ runtime, body, res, json }) {
      const value = await body()
      if (!value || !['background', 'inpaint'].includes(value.operation))
        throw workflowImageError('workflow_image_invalid')
      const reference = parseWorkflowMedia(value.reference)
      const image = normalizeWorkflowImageSettings(value.image)
      const controller = new AbortController()
      const abort = () => {
        if (!res.writableEnded) controller.abort()
      }
      res.once('close', abort)
      const runId = randomUUID()
      try {
        const context = {
          workflowId: runId,
          runId,
          inputs: { reference },
          signal: controller.signal,
        }
        const source = await runtime.workflowImageNodes.execute({
          ...context,
          node: { id: 'input', kind: 'media-input', image: { inputName: 'reference' } },
          predecessors: [],
        })
        const output = await runtime.workflowImageNodes.execute({
          ...context,
          node: {
            id: 'process',
            kind: value.operation === 'background' ? 'media-background' : 'media-inpaint',
            image,
          },
          predecessors: [source],
        })
        json(200, output.output.frames[0].media)
      } finally {
        res.removeListener('close', abort)
      }
    },
  },
  {
    method: 'POST',
    path: '/api/workflows/:workflowId/nodes/:nodeId/run',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      if (!input || typeof input.sourceRunId !== 'string' || input.sourceRunId.length > 80)
        return json(400, {
          error: 'workflow_image_source_stale',
          code: 'workflow_image_source_stale',
        })
      const run = await runtime.workflows.runNow(params.workflowId, {
        nodeId: params.nodeId,
        sourceRunId: input.sourceRunId,
      })
      if (!run) return json(404, { error: '工作流不存在。' })
      json(202, { started: true, run })
    },
  },
  {
    method: 'GET',
    path: '/api/workflow-image-models',
    async handler({ runtime, json }) {
      const status = await runtime.visualGeneration.getModelStatus('image')
      json(200, {
        models: status.models.map((model) => ({
          id: `${model.providerId}/${model.id}`,
          name: model.name,
          providerId: model.providerId,
          providerName: model.providerName,
        })),
      })
    },
  },
  {
    method: 'GET',
    path: '/api/workflows/:workflowId/bundle',
    async handler({ runtime, params, json, res }) {
      const workflow = runtime.exportWorkflow(params.workflowId)
      if (!workflow) return json(404, { error: '工作流不存在。' })
      const buffer = await exportPortableWorkflowBundle(
        workflow,
        runtime.workflowMedia,
        runtime.spriteEngines,
      )
      res.writeHead(200, {
        'Content-Type': 'application/zip',
        'Content-Length': buffer.length,
        'Cache-Control': 'no-store',
      })
      res.end(buffer)
    },
  },
  {
    method: 'POST',
    path: '/api/workflows/import-bundle',
    async handler({ runtime, bodyBuffer, json }) {
      const imported = await importPortableWorkflowBundle(
        await bodyBuffer(MAX_WORKFLOW_BUNDLE_BYTES),
        runtime.workflowMedia,
        runtime.spriteEngines,
      )
      let workflow
      try {
        workflow = await runtime.workflows.importWorkflow(imported.definition)
      } catch (error) {
        await runtime.workflowMedia.discardImported(imported.mediaMapping)
        throw error
      }
      // 领域提交成功后，响应写入或目录刷新失败都不能撤销已被工作流引用的媒体。
      json(201, { workflow, requirements: imported.requirements })
    },
  },
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
