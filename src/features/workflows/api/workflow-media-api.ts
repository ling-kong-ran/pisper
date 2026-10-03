import { parseWorkflowMedia } from '@shared/workflow/workflow-inputs.mjs'
import { requestBlob, requestJson } from '@/lib/http/http'
import { invalidResponseError } from '@/lib/http/http-response'

export const workflowMediaApi = {
  upload: (file: File, signal?: AbortSignal) =>
    requestJson(`/api/workflow-media?name=${encodeURIComponent(file.name)}`, {
      method: 'POST',
      data: file,
      headers: { 'Content-Type': file.type },
      signal,
      timeout: 120_000,
      parse: (value: unknown) => {
        try {
          return parseWorkflowMedia(value)
        } catch {
          throw invalidResponseError()
        }
      },
    }),
  preview: (id: string, signal?: AbortSignal) =>
    requestBlob(`/api/workflow-media/${encodeURIComponent(id)}/content`, { signal }),
}
