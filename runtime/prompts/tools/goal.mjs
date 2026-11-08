// get_goal / update_goal 的说明。
export const GET_GOAL = Object.freeze({
  description: 'Read the active Goal: objective, status and remaining token budget.',
  snippet: 'Read the current Goal objective and progress while pursuing it',
  guidelines: [
    'Use get_goal only when you need Goal details; Goal continuation messages already include the objective and budget.',
  ],
})

export const UPDATE_GOAL = Object.freeze({
  description:
    'Mark the active Goal complete after verifying every requirement with concrete evidence.',
  snippet: 'Mark the active Goal complete after a strict evidence-based completion audit',
  guidelines: [
    'Call update_goal only when every explicit Goal requirement is complete and verified with real evidence.',
    'Not because work seems plausible, tests partially pass, progress is substantial, or the budget is low.',
  ],
  params: { status: 'Only complete is accepted.' },
  invalidStatus: 'update_goal only accepts status=complete.',
})
