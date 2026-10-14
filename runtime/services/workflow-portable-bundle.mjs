import { validateWorkflowInputDefinitions } from '../../shared/workflow-inputs.mjs'
import {
  isWorkflowImageNodeKind,
  normalizeWorkflowImageSettings,
} from '../../shared/workflow-image-nodes.mjs'
import {
  bundleError,
  bundleJson,
  decodeWorkflowBundle,
  encodeWorkflowBundle,
  jsonBundleFile,
} from './workflow-bundle-archive.mjs'

/** @typedef {Pick<import('./workflow-media-service.mjs').WorkflowMediaService, 'exportFilesForWorkflow' | 'validateBundleFiles' | 'importBundleFiles'>} Media */
/** @typedef {Pick<import('./sprite-engine-service.mjs').SpriteEngineService, 'exportFiles' | 'validateBundleFiles' | 'installBundleFiles'>} Engines */
/** @param {unknown} value @returns {Record<string,unknown>} */
function record(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw bundleError()
  return /** @type {Record<string,unknown>} */ (value)
}
/** @param {unknown} value */
function definition(value) {
  const input = record(value)
  if (input.format !== 'pisper-workflow' || input.version !== 1) throw bundleError()
  const workflow = record(input.workflow)
  if (
    typeof workflow.name !== 'string' ||
    !Array.isArray(workflow.nodes) ||
    workflow.nodes.length > 100 ||
    !Array.isArray(workflow.edges)
  )
    throw bundleError()
  return {
    format: 'pisper-workflow',
    version: 1,
    workflow: {
      ...workflow,
      model: workflow.model,
      notifications: workflow.notifications,
      cwd: '',
      inputs: validateWorkflowInputDefinitions(workflow.inputs),
      nodes: workflow.nodes.map((value) => {
        const node = record(value)
        return isWorkflowImageNodeKind(node.kind)
          ? { ...node, image: normalizeWorkflowImageSettings(node.image) }
          : node
      }),
    },
  }
}
/** @param {unknown} value */
function modelId(value) {
  if (!value) return ''
  const model = record(value)
  if (typeof model.provider !== 'string' || typeof model.model !== 'string') throw bundleError()
  return `${model.provider}/${model.model}`
}
/** @param {unknown} value */
function strings(value) {
  return Array.isArray(value) ? value.filter((item) => typeof item === 'string') : []
}
/** @param {ReturnType<typeof definition>} input */
function requirements(input) {
  const workflow = input.workflow
  return {
    models: [
      ...new Set(
        [workflow.model, ...workflow.nodes.map((node) => node.model)].map(modelId).filter(Boolean),
      ),
    ],
    skills: [
      ...new Set(
        workflow.nodes.flatMap((node) =>
          typeof node.skillName === 'string' && node.skillName ? [node.skillName] : [],
        ),
      ),
    ],
    tools: [...new Set(workflow.nodes.flatMap((node) => strings(node.requestedToolNames)))],
    notifications: strings(workflow.notifications),
    engines: [
      ...new Set(
        workflow.nodes.flatMap((node) =>
          node.kind === 'media-inpaint'
            ? ['inpaint']
            : node.kind === 'media-background' &&
                normalizeWorkflowImageSettings(node.image).method === 'model'
              ? ['background']
              : [],
        ),
      ),
    ],
    requiresWorkspaceSelection: true,
  }
}

// 提示词、输入定义和默认素材可移植；外部模型/MCP/Skill 通过依赖清单提示目标机配置。
/** @param {unknown} input @param {Media} media @param {Engines} [engines] */
export async function exportPortableWorkflowBundle(input, media, engines) {
  const parsed = definition(structuredClone(input))
  const required = requirements(parsed)
  const engineFiles =
    required.engines.length && engines
      ? Object.fromEntries(
          Object.entries(await engines.exportFiles()).filter(([name]) =>
            required.engines.some((id) => name.startsWith(`engines/${id}/`)),
          ),
        )
      : {}
  if (
    required.engines.some(
      (id) => !Object.keys(engineFiles).some((name) => name.startsWith(`engines/${id}/`)),
    )
  )
    throw bundleError()
  return encodeWorkflowBundle({
    'manifest.json': jsonBundleFile({
      format: 'pisper-workflow-bundle',
      version: 1,
      kind: 'dag',
      requirements: required,
    }),
    'workflow.json': jsonBundleFile(parsed),
    ...(await media.exportFilesForWorkflow(parsed.workflow)),
    ...engineFiles,
    'README.txt': new TextEncoder().encode(
      'Pisper workflow package\nThe graph, node settings, input definitions, default media and required downloaded image algorithms are included. API credentials are never exported.\nBefore running, choose a workspace and configure the external models, skills, tools and notification services listed in manifest.json on the destination machine.\nLocal color-key and frame processing work offline; external image generation still needs a configured model.\n',
    ),
  })
}

/** @param {Uint8Array} buffer @param {Media} media @param {Engines} [engines] */
export async function importPortableWorkflowBundle(buffer, media, engines) {
  const files = decodeWorkflowBundle(buffer)
  if (
    Object.keys(files).some(
      (name) =>
        !['manifest.json', 'workflow.json', 'README.txt'].includes(name) &&
        !name.startsWith('media/') &&
        !name.startsWith('engines/'),
    )
  )
    throw bundleError()
  const manifest = record(bundleJson(files, 'manifest.json'))
  if (
    manifest.format !== 'pisper-workflow-bundle' ||
    manifest.version !== 1 ||
    manifest.kind !== 'dag'
  )
    throw bundleError()
  const parsed = definition(bundleJson(files, 'workflow.json'))
  // 所有元数据检查先完成，不能在素材落盘后才发现无效的模型依赖。
  const dependenciesRequired = requirements(parsed)
  const engineFiles = Object.fromEntries(
    Object.entries(files).filter(([name]) => name.startsWith('engines/')),
  )
  if (
    Object.keys(engineFiles).some(
      (name) => !dependenciesRequired.engines.some((id) => name.startsWith(`engines/${id}/`)),
    ) ||
    dependenciesRequired.engines.some(
      (id) => !Object.keys(engineFiles).some((name) => name.startsWith(`engines/${id}/`)),
    )
  )
    throw bundleError()
  if (dependenciesRequired.engines.length) {
    if (!engines) throw bundleError()
    engines.validateBundleFiles(engineFiles)
  }
  const dependencies = Object.fromEntries(
    Object.entries(files).filter(([name]) => name.startsWith('media/')),
  )
  const entries = media.validateBundleFiles(dependencies)
  const referenced = parsed.workflow.inputs.flatMap((input) =>
    typeof input.defaultValue === 'object' && input.defaultValue ? [input.defaultValue] : [],
  )
  const available = new Map(entries.map(({ metadata }) => [metadata.media.id, metadata.media]))
  if (
    entries.length !== new Set(referenced.map((media) => media.id)).size ||
    referenced.some((media) => JSON.stringify(available.get(media.id)) !== JSON.stringify(media))
  )
    throw bundleError()
  // 算法是独立的已校验共享缓存；模板提交失败不会卸载其他工作流可复用的依赖。
  if (dependenciesRequired.engines.length && engines) await engines.installBundleFiles(engineFiles)
  const mapping = await media.importBundleFiles(dependencies)
  parsed.workflow.inputs = parsed.workflow.inputs.map((input) => ({
    ...input,
    defaultValue:
      input.defaultValue && typeof input.defaultValue === 'object'
        ? mapping[input.defaultValue.id]
        : input.defaultValue,
  }))
  return { definition: parsed, requirements: dependenciesRequired, mediaMapping: mapping }
}
