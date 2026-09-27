// DAG 适配器只把节点输入映射为图像插件调用，不拥有图像引擎或任务生命周期。
import {
  isWorkflowImageNodeKind,
  normalizeWorkflowImageSettings,
  parseWorkflowImageOutput,
  workflowImageError,
} from '../../shared/workflow-image-nodes.mjs'
import { parseWorkflowMedia } from '../../shared/workflow-inputs.mjs'
export class WorkflowImageNodeService {
  /** @param {{operations: Pick<import('./image-operation-service.mjs').ImageOperationService,'execute'>}} dependencies */
  constructor({ operations }) {
    this.operations = operations
  }
  /** @param {{node:{kind:string,image?:unknown,prompt?:string,model?:{provider:string,model:string}|null},inputs:Record<string,unknown>,predecessors:Array<{output?:unknown}>,signal?:AbortSignal,resumeOutput?:unknown}} request */
  execute({ node, inputs, predecessors, signal, resumeOutput }) {
    if (!isWorkflowImageNodeKind(node.kind)) throw workflowImageError('workflow_image_invalid')
    const settings = normalizeWorkflowImageSettings(node.image)
    const kinds = /** @type {const} */ ({
      'media-input': 'input',
      'media-background': 'background',
      'media-inpaint': 'inpaint',
      'media-generate': 'generate',
      'media-frames': 'frames',
      'media-transform': 'transform',
      'media-preview': 'preview',
      'media-export': 'export',
    })
    return this.operations.execute({
      operation: kinds[node.kind],
      settings,
      ...(node.kind === 'media-input'
        ? { source: parseWorkflowMedia(inputs[settings.inputName]) }
        : {
            images: predecessors.flatMap(({ output }) => parseWorkflowImageOutput(output).frames),
          }),
      prompt: node.prompt,
      model: node.model,
      signal,
      resumeOutput,
    })
  }
}
