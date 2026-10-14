export {
  IMAGE_DIRECTIONS as WORKFLOW_IMAGE_DIRECTIONS,
  normalizeImageSettings as normalizeWorkflowImageSettings,
  parseImageOutput as parseWorkflowImageOutput,
  imageOperationError as workflowImageError,
} from './image-operations.mjs'
export type {
  ImageSettings as WorkflowImageSettings,
  ImageFrame as WorkflowImageFrame,
  ImageOutput as WorkflowImageOutput,
  ImageFrameTransform as WorkflowFrameTransform,
  ImageDirection as WorkflowDirection,
} from './image-operations.mjs'
export type WorkflowImageNodeKind =
  | 'media-input'
  | 'media-background'
  | 'media-inpaint'
  | 'media-generate'
  | 'media-frames'
  | 'media-transform'
  | 'media-preview'
  | 'media-export'

export const WORKFLOW_IMAGE_NODE_KINDS: readonly WorkflowImageNodeKind[]
export function isWorkflowImageNodeKind(value: unknown): value is WorkflowImageNodeKind
