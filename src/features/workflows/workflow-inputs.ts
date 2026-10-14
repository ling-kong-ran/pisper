import {
  parseWorkflowMedia,
  validateWorkflowInputDefinitions,
  validateWorkflowInputs,
  WorkflowInputError,
} from '@shared/workflow-inputs.mjs'
import type { WorkflowInputValue } from '@shared/workflow-inputs.mjs'
import type { WorkflowInput, WorkflowInputType } from './types'
import type { WorkflowTranslate } from './workflow-templates'

export type { WorkflowInputValue } from '@shared/workflow-inputs.mjs'
export type WorkflowInputValues = Record<string, WorkflowInputValue>

export function workflowInputType(value: string): WorkflowInputType {
  return value === 'number' ||
    value === 'boolean' ||
    value === 'text' ||
    value === 'image' ||
    value === 'video'
    ? value
    : 'string'
}

export function workflowInputDefault(input: WorkflowInput): WorkflowInputValue {
  if (input.type === 'image' || input.type === 'video') {
    try {
      return parseWorkflowMedia(input.defaultValue)
    } catch {
      return null
    }
  }
  if (input.type === 'boolean') return input.defaultValue === true || input.defaultValue === 'true'
  if (typeof input.defaultValue === 'number' || typeof input.defaultValue === 'string')
    return input.defaultValue
  return ''
}

function inputErrorMessage(error: unknown, t: WorkflowTranslate) {
  if (!(error instanceof WorkflowInputError)) return t('workflows:inputs.invalidDefinition')
  const name = error.label || error.inputName
  if (error.code === 'workflow_input_unsafe_name') return t('workflows:inputs.invalidName')
  if (error.code === 'workflow_input_duplicate_name')
    return t('workflows:inputs.duplicateName', { name })
  if (error.code === 'workflow_input_required') return t('workflows:inputs.requiredValue', { name })
  if (error.code === 'workflow_input_type') return t('workflows:inputs.invalidValue', { name })
  if (error.code === 'workflow_input_too_long') return t('workflows:inputs.valueTooLong', { name })
  if (error.code === 'workflow_input_unknown') return t('workflows:inputs.unknownInput', { name })
  return t('workflows:inputs.invalidDefinition')
}

export function workflowInputDefinitionError(inputs: WorkflowInput[], t: WorkflowTranslate) {
  try {
    validateWorkflowInputDefinitions(inputs)
    return ''
  } catch (error) {
    return inputErrorMessage(error, t)
  }
}

export function workflowRunInputError(
  inputs: WorkflowInput[],
  values: WorkflowInputValues,
  t: WorkflowTranslate,
) {
  try {
    validateWorkflowInputs(inputs, values)
    return ''
  } catch (error) {
    return inputErrorMessage(error, t)
  }
}

export function workflowRunInputValues(
  inputs: WorkflowInput[],
  values: WorkflowInputValues,
): WorkflowInputValues {
  return validateWorkflowInputs(inputs, values)
}
