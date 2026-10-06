import { requestJson } from '@/lib/http/http'
import { invalidResponseError } from '@/lib/http/http-response'

export type WorkflowImageModel = {
  id: string
  name: string
  providerId: string
  providerName: string
}

function parseModels(value: unknown): WorkflowImageModel[] {
  if (!value || typeof value !== 'object' || !('models' in value) || !Array.isArray(value.models))
    throw invalidResponseError()
  return value.models.map((model: unknown) => {
    if (
      !model ||
      typeof model !== 'object' ||
      !('id' in model) ||
      typeof model.id !== 'string' ||
      !('name' in model) ||
      typeof model.name !== 'string' ||
      !('providerId' in model) ||
      typeof model.providerId !== 'string' ||
      !('providerName' in model) ||
      typeof model.providerName !== 'string' ||
      !model.id.startsWith(`${model.providerId}/`)
    )
      throw invalidResponseError()
    return {
      id: model.id,
      name: model.name,
      providerId: model.providerId,
      providerName: model.providerName,
    }
  })
}

function parseStarted(value: unknown) {
  if (
    !value ||
    typeof value !== 'object' ||
    !('started' in value) ||
    value.started !== true ||
    !('run' in value) ||
    !value.run ||
    typeof value.run !== 'object' ||
    !('id' in value.run) ||
    typeof value.run.id !== 'string' ||
    !('workflowId' in value.run) ||
    typeof value.run.workflowId !== 'string'
  )
    throw invalidResponseError()
  return { started: true, run: { id: value.run.id, workflowId: value.run.workflowId } }
}

export const workflowImageApi = {
  runNode: (workflowId: string, nodeId: string, sourceRunId: string, signal?: AbortSignal) =>
    requestJson(
      `/api/workflows/${encodeURIComponent(workflowId)}/nodes/${encodeURIComponent(nodeId)}/run`,
      { method: 'POST', data: { sourceRunId }, signal, parse: parseStarted },
    ),
  models: (signal?: AbortSignal) =>
    requestJson('/api/workflow-image-models', { signal, parse: parseModels }),
}
