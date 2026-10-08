<<<<<<<< HEAD:shared/workflow/workflow-image-nodes.mjs
// 工作流节点协议适配；图片计算模型由独立的通用图像层维护。
export {
  IMAGE_DIRECTIONS as WORKFLOW_IMAGE_DIRECTIONS,
  normalizeImageSettings as normalizeWorkflowImageSettings,
  parseImageOutput as parseWorkflowImageOutput,
  imageOperationError as workflowImageError,
} from '../image/image-operations.mjs'
/** @type {readonly import('./workflow-image-nodes.mjs').WorkflowImageNodeKind[]} */
export const WORKFLOW_IMAGE_NODE_KINDS = Object.freeze([
  'media-input',
  'media-background',
  'media-inpaint',
  'media-generate',
  'media-frames',
  'media-transform',
  'media-preview',
  'media-export',
])
/** @param {unknown} value @returns {value is import('./workflow-image-nodes.mjs').WorkflowImageNodeKind} */
export function isWorkflowImageNodeKind(value) {
  return typeof value === 'string' && WORKFLOW_IMAGE_NODE_KINDS.some((kind) => kind === value)
}
========
// 旧 Rust 分支构建脚本的兼容入口；业务协议以 release 的分域模块为唯一来源。
export * from './workflow/workflow-image-nodes.mjs'
>>>>>>>> origin/develop-rust:shared/workflow-image-nodes.mjs
