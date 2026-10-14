import { ApiError, requestBlob, requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'

export function validateWorkflowPackageSize(file: File) {
  if (file.size > 128 * 1024 * 1024)
    throw new ApiError('Workflow package exceeds 128 MiB', {
      kind: 'protocol',
      data: { code: 'workflow_bundle_too_large' },
    })
}

export function isWorkflowPackageTooLarge(error: unknown) {
  return error instanceof ApiError && error.data?.code === 'workflow_bundle_too_large'
}

export function exportWorkflowPackage(id: string, signal?: AbortSignal) {
  return requestBlob(`/api/workflows/${encodeURIComponent(id)}/bundle`, { signal, timeout: 120000 })
}

export async function importWorkflowPackage(file: File, signal?: AbortSignal) {
  validateWorkflowPackageSize(file)
  return requestJson('/api/workflows/import-bundle', {
    method: 'POST',
    data: file,
    signal,
    timeout: 120000,
    parse: (value: unknown) => {
      if (!value || typeof value !== 'object' || !('workflow' in value))
        throw invalidResponseError()
      const workflow = value.workflow
      if (
        !workflow ||
        typeof workflow !== 'object' ||
        !('id' in workflow) ||
        typeof workflow.id !== 'string'
      )
        throw invalidResponseError()
    },
  })
}
