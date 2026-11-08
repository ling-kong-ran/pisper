import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { GET_GOAL, UPDATE_GOAL } from '../../prompts/tools/goal.mjs'

// 目标工具：get_goal / update_goal（证据审计后完成目标）。
export const GOAL_TOOL_NAMES = Object.freeze(['get_goal', 'update_goal'])

export function createGoalTools({ getGoal, completeGoal }) {
  return [
    defineTool({
      name: 'get_goal',
      label: 'Get Goal',
      description: GET_GOAL.description,
      promptSnippet: GET_GOAL.snippet,
      promptGuidelines: GET_GOAL.guidelines,
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
      description: UPDATE_GOAL.description,
      promptSnippet: UPDATE_GOAL.snippet,
      promptGuidelines: UPDATE_GOAL.guidelines,
      parameters: Type.Object({
        status: Type.Literal('complete', { description: UPDATE_GOAL.params.status }),
      }),
      async execute(_toolCallId, params) {
        if (params.status !== 'complete')
          return {
            content: [{ type: 'text', text: UPDATE_GOAL.invalidStatus }],
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
