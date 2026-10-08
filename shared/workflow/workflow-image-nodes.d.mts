<<<<<<<< HEAD:shared/workflow/workflow-image-nodes.d.mts
export {
  IMAGE_DIRECTIONS as WORKFLOW_IMAGE_DIRECTIONS,
  normalizeImageSettings as normalizeWorkflowImageSettings,
  parseImageOutput as parseWorkflowImageOutput,
  imageOperationError as workflowImageError,
} from '../image/image-operations.mjs'
export type {
  ImageSettings as WorkflowImageSettings,
  ImageFrame as WorkflowImageFrame,
  ImageOutput as WorkflowImageOutput,
  ImageFrameTransform as WorkflowFrameTransform,
  ImageDirection as WorkflowDirection,
} from '../image/image-operations.mjs'
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
========
// 旧 Rust 分支构建脚本的兼容入口；业务协议以 release 的分域模块为唯一来源。
export * from './workflow/workflow-image-nodes.mjs'
>>>>>>>> origin/develop-rust:shared/workflow-image-nodes.d.mts
