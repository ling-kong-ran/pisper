export type WorkflowActions = {
  busy: boolean
  running: boolean
  save: () => void | Promise<unknown>
  run: () => void | Promise<unknown>
}
