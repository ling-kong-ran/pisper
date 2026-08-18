import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'

// 目标工具：get_goal / update_goal（证据审计后完成目标）。
export const GOAL_TOOL_NAMES = Object.freeze(['get_goal', 'update_goal'])

export function createGoalTools({ getGoal, completeGoal }) {
  return [
    defineTool({
      name: 'get_goal',
      label: 'Get Goal',
      description: 'Read the current active Goal objective, status, and remaining token budget.',
      promptSnippet: 'Read the current Goal objective and progress while pursuing it',
      promptGuidelines: [
        'Use get_goal only when the current Goal details are needed; active Goal continuation messages already include the objective and budget.',
      ],
      parameters: Type.Object({}),
      async execute() {
        const goal = await getGoal?.()
        return {
          content: [{ type: 'text', text: JSON.stringify({ goal }, null, 2) }],
          details: { goal },
        }
      },
    }),
    defineTool({
      name: 'update_goal',
      label: 'Complete Goal',
      description:
        'Mark the current active Goal complete after verifying every requirement with concrete evidence.',
      promptSnippet: 'Mark the active Goal complete after a strict evidence-based completion audit',
      promptGuidelines: [
        'Call update_goal only when every explicit Goal requirement is complete and verified with real evidence.',
        'Do not use it merely because work seems plausible, tests partially pass, progress is substantial, or the token budget is low.',
      ],
      parameters: Type.Object({
        status: Type.Literal('complete', { description: 'Only complete is accepted.' }),
      }),
      async execute(_toolCallId, params) {
        if (params.status !== 'complete')
          return {
            content: [{ type: 'text', text: 'update_goal only accepts status=complete.' }],
            isError: true,
          }
        const goal = await completeGoal?.()
        return {
          content: [{ type: 'text', text: JSON.stringify({ goal }, null, 2) }],
          details: { goal },
        }
      },
    }),
  ]
}
