import type { WorkflowMedia } from '../workflow/workflow-inputs.mjs'
import type { ImageDirection, ImageOutput } from '../image/image-operations.mjs'
import type { ImageFrameEdits } from '../image/image-frame-edits.mjs'

export type GameAssetAction = { id: string; name: string; prompt: string; enabled: boolean }
export type GameAssetProjectInput = {
  id?: string
  name: string
  prompt: string
  reference: WorkflowMedia | null
  originalReference: WorkflowMedia | null
  frameCount: number
  directions: ImageDirection[]
  model: { provider: string; model: string } | null
  actions: GameAssetAction[]
}
export type GameAssetProject = GameAssetProjectInput & {
  id: string
  createdAt: string
  updatedAt: string
}
export type GameAssetJob = {
  id: string
  projectId: string
  status: 'running' | 'completed' | 'failed' | 'cancelled' | 'interrupted'
  startedAt: string
  finishedAt: string | null
  completed: number
  total: number
  error: string | null
  output: ImageOutput
  originalOutput: ImageOutput
  edits?: ImageFrameEdits
  revision: number
}
export type GameAssetsCatalog = { projects: GameAssetProject[]; jobs: GameAssetJob[] }
export class GameAssetError extends Error {
  code: string
  statusCode: number
  constructor(code: string, statusCode?: number)
}
export function parseGameAssetProjectInput(value: unknown): GameAssetProjectInput
export function parseGameAssetProject(value: unknown): GameAssetProject
export function parseGameAssetJob(value: unknown): GameAssetJob
export function parseGameAssetsCatalog(value: unknown): GameAssetsCatalog
