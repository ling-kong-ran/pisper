export type WorkflowMedia = { id: string; name: string; mimeType: string; size: number }
export type WorkflowInputValue = string | number | boolean | WorkflowMedia | null
export type WorkflowInputDefinition = {
  id: string
  name: string
  label: string
  type: 'string' | 'text' | 'number' | 'boolean' | 'image' | 'video'
  required: boolean
  defaultValue: WorkflowInputValue
  description: string
}
export const WORKFLOW_INPUT_MAX_LENGTH: number
export class WorkflowInputError extends Error {
  code: string
  inputName: string
  label: string
  statusCode: number
  constructor(code: string, inputName?: string, label?: string)
}
export function parseWorkflowMedia(value: unknown): WorkflowMedia
export function validateWorkflowInputDefinitions(value: unknown): WorkflowInputDefinition[]
export function validateWorkflowInputs(
  definitions: unknown,
  supplied?: unknown,
): Record<string, WorkflowInputValue>
export function validateWorkflowTemplate(
  template: string,
  inputNames: string[],
  nodeIds: string[],
): void
export function renderWorkflowTemplate(template: string, context: Record<string, unknown>): string
