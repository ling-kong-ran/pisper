import { defineTool } from '@earendil-works/pi-coding-agent'
import { Type } from 'typebox'
import { MAX_TASK_ASSIGNEE_CHARS, MAX_TASK_DEPENDS_ON, MAX_TASK_LIST_ITEMS, MAX_TASK_NOTE_CHARS, MAX_TASK_TITLE_CHARS, TASK_LIST_STATUSES } from '../../services/task-list-service.mjs'

export const TASK_LIST_TOOL_NAMES = Object.freeze(['get_task_list', 'update_task_list'])

const statusSchema = Type.String({ enum: TASK_LIST_STATUSES })

export function createTaskListTools({ getTaskList, updateTaskList }) {
  return [
    defineTool({
      name: 'get_task_list',
      label: 'Get Task List',
      description: 'Read the current primary Agent session task list and progress counts.',
      promptSnippet: 'Read the structured task list for the current primary Agent session',
      promptGuidelines: [
        'Use get_task_list when the current task breakdown is needed and has not already been returned by update_task_list.',
        'This list belongs to the primary Agent session. Subagents may read it for coordination but cannot modify it.',
      ],
      parameters: Type.Object({}),
      async execute() {
        const taskList = await getTaskList?.()
        return { content: [{ type: 'text', text: JSON.stringify({ taskList }, null, 2) }], details: { taskList } }
      },
    }),
    defineTool({
      name: 'update_task_list',
      label: 'Update Task List',
      description: 'Replace the current primary Agent session task list with a structured progress snapshot.',
      promptSnippet: 'Create and maintain a concise structured task list for multi-step work',
      promptGuidelines: [
        'Use update_task_list for work with multiple concrete steps or when the user explicitly asks for a task list.',
        'Keep stable task ids when updating status. Preserve unfinished tasks unless they are genuinely removed from scope.',
        'Set status to in_progress before substantive work, completed only after verification, and blocked only when a concrete blocker exists.',
        'Keep the list concise and outcome-oriented. Do not create a task for trivial narration or every individual tool call.',
        'An empty items array clears the task list.',
        'Only the primary Agent may modify this list. Subagents have read-only access for coordination.',
      ],
      parameters: Type.Object({
        items: Type.Array(Type.Object({
          id: Type.Optional(Type.String({ minLength: 1, maxLength: 80, description: 'Stable task id reused across updates' })),
          title: Type.String({ minLength: 1, maxLength: MAX_TASK_TITLE_CHARS }),
          status: statusSchema,
          note: Type.Optional(Type.String({ maxLength: MAX_TASK_NOTE_CHARS })),
          assignee: Type.Optional(Type.String({ maxLength: MAX_TASK_ASSIGNEE_CHARS, description: 'Who owns this task: an agent canonical name, or empty when unassigned' })),
          dependsOn: Type.Optional(Type.Array(Type.String({ minLength: 1, maxLength: 80 }), { maxItems: MAX_TASK_DEPENDS_ON, description: 'Existing task ids that must complete first; self-references and cycles are rejected' })),
        }), { maxItems: MAX_TASK_LIST_ITEMS }),
      }),
      async execute(_toolCallId, params) {
        const taskList = await updateTaskList?.(params.items)
        return { content: [{ type: 'text', text: JSON.stringify({ taskList }, null, 2) }], details: { taskList } }
      },
    }),
  ]
}
