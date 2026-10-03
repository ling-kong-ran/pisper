// 工作流模板：新建工作流时可选的起点模板（分类/描述/预置节点）。
import {
  Bell,
  Bot,
  Braces,
  CircleCheck,
  Code2,
  File,
  FileCode2,
  GitBranch,
  Image,
  Network,
  Rocket,
  Search,
  Server,
  Zap,
} from 'lucide-react'
import {
  isWorkflowImageNodeKind,
  normalizeWorkflowImageSettings,
  WORKFLOW_IMAGE_DIRECTIONS,
} from '@shared/workflow/workflow-image-nodes.mjs'
import { createLinearWorkflowEdges } from '@shared/workflow/workflow-graph.mjs'
import type { I18nValues } from '@/app/i18n/i18n'
import type { LucideIcon } from 'lucide-react'
import type { NodeKind, Workflow, WorkflowInput, WorkflowNode, WorkflowEdge } from './types'

export type WorkflowTranslate = (message: string, values?: I18nValues) => string
export type WorkflowTemplate = {
  id: string
  name: string
  description: string
  Icon: LucideIcon
  nodes: WorkflowNode[]
  inputs: WorkflowInput[]
  edges?: WorkflowEdge[]
}

export const WORKFLOW_FILTERS = ['all', 'presets', 'custom', 'running', 'failed', 'draft'] as const
export type WorkflowFilter = (typeof WORKFLOW_FILTERS)[number]

export const NODE_TYPE_NAMES: Record<NodeKind, string> = {
  trigger: '触发器',
  prompt: '任务',
  skill: 'Skill',
  file: '文件',
  mcp: 'MCP',
  notification: '通知',
  condition: '判断',
  parallel: '并行',
  approval: '审批',
  'media-input': '图片输入',
  'media-inpaint': '区域修补',
  'media-background': '去背景',
  'media-generate': '动作生成',
  'media-frames': '拆分帧',
  'media-transform': '帧处理',
  'media-preview': '动画预览',
  'media-export': '图集输出',
}

export const WORKFLOW_PALETTE = [
  { kind: 'trigger', label: '手动触发', Icon: Zap },
  { kind: 'prompt', label: '运行 Prompt', Icon: Bot },
  { kind: 'skill', label: '调用 Skill', Icon: Braces },
  { kind: 'file', label: '读写文件', Icon: FileCode2 },
  { kind: 'mcp', label: '调用 MCP', Icon: Server },
  { kind: 'condition', label: '条件分支', Icon: GitBranch },
  { kind: 'parallel', label: '并行汇合', Icon: Network },
  { kind: 'approval', label: '人工审批', Icon: CircleCheck },
  { kind: 'notification', label: '发送通知', Icon: Bell },
  { kind: 'media-inpaint', label: '区域修补', Icon: Image },
  { kind: 'media-input', label: '图片输入', Icon: Image },
  { kind: 'media-background', label: '去背景', Icon: Image },
  { kind: 'media-generate', label: '动作生成', Icon: Image },
  { kind: 'media-frames', label: '拆分帧', Icon: Image },
  { kind: 'media-transform', label: '帧处理', Icon: Image },
  { kind: 'media-preview', label: '动画预览', Icon: Image },
  { kind: 'media-export', label: '图集输出', Icon: Image },
] satisfies Array<{ kind: NodeKind; label: string; Icon: LucideIcon }>

export function workflowFilterLabel(filter: WorkflowFilter, t: WorkflowTranslate) {
  if (filter === 'presets') return t('workflows:workflowsPage.presets')
  if (filter === 'custom') return t('workflows:workflowsPage.custom')
  if (filter === 'running') return t('workflows:workflowsPage.running')
  if (filter === 'failed') return t('workflows:workflowsPage.failed')
  if (filter === 'draft') return t('workflows:workflowsPage.draft')
  return t('workflows:workflowsPage.all')
}

export function nodeTypeLabel(kind: NodeKind, t: WorkflowTranslate) {
  if (kind === 'media-inpaint') return t('workflows:imageNodes.inpaint')
  if (kind === 'media-input') return t('workflows:imageNodes.input')
  if (kind === 'media-background') return t('workflows:imageNodes.background')
  if (kind === 'media-generate') return t('workflows:imageNodes.generate')
  if (kind === 'media-frames') return t('workflows:imageNodes.frames')
  if (kind === 'media-transform') return t('workflows:imageNodes.transform')
  if (kind === 'media-preview') return t('workflows:imageNodes.preview')
  if (kind === 'media-export') return t('workflows:imageNodes.export')
  if (kind === 'trigger') return t('workflows:workflowsPage.triggerNode')
  if (kind === 'skill') return t('workflows:workflowsPage.skillNode')
  if (kind === 'file') return t('workflows:workflowsPage.fileNode')
  if (kind === 'mcp') return t('workflows:workflowsPage.mcpNode')
  if (kind === 'notification') return t('workflows:workflowsPage.notificationNode')
  if (kind === 'condition') return t('workflows:workflowsPage.conditionNode')
  if (kind === 'parallel') return t('workflows:workflowsPage.parallelNode')
  if (kind === 'approval') return t('workflows:workflowsPage.approvalNode')
  return t('workflows:workflowsPage.task')
}

export function paletteLabel(kind: NodeKind, t: WorkflowTranslate) {
  if (isWorkflowImageNodeKind(kind)) return nodeTypeLabel(kind, t)
  if (kind === 'trigger') return t('workflows:workflowsPage.manualTrigger')
  if (kind === 'skill') return t('workflows:workflowsPage.callSkill')
  if (kind === 'file') return t('workflows:workflowsPage.readWriteFiles')
  if (kind === 'mcp') return t('workflows:workflowsPage.callMcp')
  if (kind === 'condition') return t('workflows:workflowsPage.conditionBranch')
  if (kind === 'parallel') return t('workflows:workflowsPage.parallelJoin')
  if (kind === 'approval') return t('workflows:workflowsPage.humanApproval')
  if (kind === 'notification') return t('workflows:workflowsPage.sendNotification')
  return t('workflows:workflowsPage.runPrompt')
}

export function templateName(templateId: string, t: WorkflowTranslate) {
  if (templateId === 'sprite') return t('workflows:imageNodes.templateName')
  if (templateId === 'pr-fix') return t('workflows:workflowsPage.prFix')
  if (templateId === 'research') return t('workflows:workflowsPage.research')
  if (templateId === 'report') return t('workflows:workflowsPage.dailyWeeklyReport')
  if (templateId === 'asset') return t('workflows:workflowsPage.assetGeneration')
  if (templateId === 'release') return t('workflows:workflowsPage.releasePreparation')
  return t('workflows:workflowsPage.codeReview')
}

export function templateDescription(templateId: string, t: WorkflowTranslate) {
  if (templateId === 'sprite') return t('workflows:imageNodes.templateDescription')
  if (templateId === 'pr-fix') return t('workflows:workflowsPage.prFixDescription')
  if (templateId === 'research') return t('workflows:workflowsPage.researchDescription')
  if (templateId === 'report') return t('workflows:workflowsPage.reportDescription')
  if (templateId === 'asset') return t('workflows:workflowsPage.assetDescription')
  if (templateId === 'release') return t('workflows:workflowsPage.releaseDescription')
  return t('workflows:workflowsPage.codeReviewDescription')
}

export function createWorkflowNode(
  id: string,
  kind: NodeKind,
  label: string,
  prompt: string,
  x: number,
  y: number,
  extra: Partial<WorkflowNode> = {},
): WorkflowNode {
  return {
    id,
    kind,
    label,
    prompt,
    x,
    y,
    model: null,
    executionMode: 'full-access',
    retries: 0,
    timeoutMinutes: 20,
    failurePolicy: 'stop',
    enabled: true,
    outputFormat: 'text',
    skillName: '',
    requestedToolNames: [],
    condition: { source: 'previous', operator: 'exists', value: '' },
    approval: { message: '', timeoutMinutes: 60 },
    notification: { title: '', content: '' },
    notificationTargets: [],
    ...(isWorkflowImageNodeKind(kind) ? { image: normalizeWorkflowImageSettings() } : {}),
    ...extra,
  }
}

function linearEdges(nodes: WorkflowNode[]) {
  return createLinearWorkflowEdges(nodes, () => crypto.randomUUID())
}

function reusableInputs(t?: WorkflowTranslate): WorkflowInput[] {
  return [
    {
      id: 'input-task',
      name: 'task',
      label: t ? t('workflows:inputs.task') : '本次任务',
      description: t ? t('workflows:inputs.taskHint') : '描述这次运行需要完成的目标。',
      type: 'text',
      required: true,
      defaultValue: '',
    },
    {
      id: 'input-materials',
      name: 'materials',
      label: t ? t('workflows:inputs.materials') : '参考材料',
      description: t
        ? t('workflows:inputs.materialsHint')
        : '填写文件路径、链接、代码片段或需要处理的内容。',
      type: 'text',
      required: false,
      defaultValue: '',
    },
    {
      id: 'input-constraints',
      name: 'constraints',
      label: t ? t('workflows:inputs.constraints') : '约束与输出要求',
      description: t
        ? t('workflows:inputs.constraintsHint')
        : '说明范围、格式、禁止操作及验收条件。',
      type: 'text',
      required: false,
      defaultValue: '',
    },
  ]
}

function reusablePrompt(rule: string) {
  return `${rule}\n\n本次任务：{{inputs.task}}\n参考材料：{{inputs.materials}}\n约束与输出要求：{{inputs.constraints}}\n以本次运行输入限定任务范围，节点只规定处理方法；不要把节点示例或工作流名称当作本次任务。`
}

export const WORKFLOW_TEMPLATES: WorkflowTemplate[] = [
  {
    id: 'code-review',
    name: '代码审查',
    description: '读取 diff → 运行测试 → 生成 review',
    Icon: Code2,
    nodes: [
      createWorkflowNode('review-trigger', 'trigger', '手动触发', '', 65, 45),
      createWorkflowNode(
        'review-diff',
        'file',
        '读取 diff',
        '读取当前工作区的 git diff，识别改动范围与高风险文件。',
        235,
        45,
      ),
      createWorkflowNode(
        'review-test',
        'prompt',
        '运行检查',
        '运行适合当前项目的测试与 lint，记录失败原因。',
        405,
        45,
      ),
      createWorkflowNode(
        'review-report',
        'prompt',
        '生成 review',
        '结合 diff 和验证结果，输出按严重度排序的代码审查结论。',
        235,
        180,
      ),
      createWorkflowNode('review-notify', 'notification', '发送结果', '', 405, 180),
    ],
  },
  {
    id: 'pr-fix',
    name: 'PR 修复',
    description: '定位失败 → 修改代码 → 回归测试',
    Icon: GitBranch,
    nodes: [
      createWorkflowNode('fix-trigger', 'trigger', '手动触发', '', 65, 45),
      createWorkflowNode(
        'fix-find',
        'prompt',
        '定位失败',
        '检查项目状态与失败信息，定位最可能的根因。',
        235,
        45,
      ),
      createWorkflowNode(
        'fix-code',
        'prompt',
        '修改代码',
        '修复已定位的问题，保留用户已有改动，不执行破坏性命令。',
        405,
        45,
      ),
      createWorkflowNode(
        'fix-test',
        'prompt',
        '回归测试',
        '运行针对性测试和构建，确认修复没有引入回归。',
        235,
        180,
      ),
      createWorkflowNode('fix-notify', 'notification', '通知结果', '', 405, 180),
    ],
  },
  {
    id: 'research',
    name: '资料调研',
    description: '搜索资料 → 提取引用 → 点亮星忆',
    Icon: Search,
    nodes: [
      createWorkflowNode('research-trigger', 'trigger', '手动输入', '', 65, 45),
      createWorkflowNode(
        'research-search',
        'prompt',
        '搜索资料',
        '围绕工作流描述中的主题检索项目内资料与可用信息源。',
        235,
        45,
      ),
      createWorkflowNode(
        'research-summary',
        'prompt',
        '整理引用',
        '整理关键结论、证据、限制和下一步建议。',
        405,
        45,
      ),
      createWorkflowNode(
        'research-memory',
        'prompt',
        '保存星忆',
        '把适合长期保留的结论写入 Agent 记忆。',
        320,
        180,
      ),
    ],
  },
  {
    id: 'report',
    name: '日报周报',
    description: '汇总会话 → 生成摘要 → 渠道通知',
    Icon: File,
    nodes: [
      createWorkflowNode('report-trigger', 'trigger', '手动触发', '', 65, 45),
      createWorkflowNode(
        'report-collect',
        'prompt',
        '汇总进展',
        '汇总当前项目近期完成事项、风险与待办。',
        235,
        45,
      ),
      createWorkflowNode(
        'report-write',
        'prompt',
        '生成报告',
        '将汇总内容整理为清晰的日报或周报。',
        405,
        45,
      ),
      createWorkflowNode('report-notify', 'notification', '渠道通知', '', 320, 180),
    ],
  },
  {
    id: 'asset',
    name: '资产生成',
    description: '生成图片 → 存入资产库 → 通知验收',
    Icon: Image,
    nodes: [
      createWorkflowNode('asset-trigger', 'trigger', '手动输入', '', 65, 45),
      createWorkflowNode(
        'asset-generate',
        'prompt',
        '生成视觉资产',
        '根据工作流描述生成需要的视觉资产，并保存生成文件。',
        235,
        45,
      ),
      createWorkflowNode(
        'asset-check',
        'prompt',
        '检查产物',
        '检查生成资产是否完整、可访问并符合需求。',
        405,
        45,
      ),
      createWorkflowNode('asset-notify', 'notification', '通知验收', '', 320, 180),
    ],
  },
  {
    id: 'release',
    name: '发布准备',
    description: '版本检查 → changelog → 创建发布单',
    Icon: Rocket,
    nodes: [
      createWorkflowNode('release-trigger', 'trigger', '手动触发', '', 65, 45),
      createWorkflowNode(
        'release-check',
        'prompt',
        '版本检查',
        '检查工作区、测试、构建和版本信息是否满足发布要求。',
        235,
        45,
      ),
      createWorkflowNode(
        'release-log',
        'prompt',
        '生成 changelog',
        '根据近期提交和改动生成 changelog 与发布说明。',
        405,
        45,
      ),
      createWorkflowNode(
        'release-report',
        'prompt',
        '发布清单',
        '生成最终发布检查清单并标记阻塞项。',
        320,
        180,
      ),
    ],
  },
].map((template) => ({
  ...template,
  inputs: reusableInputs(),
  nodes: template.nodes.map((node) =>
    ['prompt', 'file', 'skill', 'mcp'].includes(node.kind)
      ? { ...node, prompt: reusablePrompt(node.prompt) }
      : node,
  ),
}))

function spriteWorkflowTemplate(): WorkflowTemplate {
  const nodes = [
    createWorkflowNode('sprite-trigger', 'trigger', '手动触发', '', 0, 180),
    createWorkflowNode('sprite-source', 'media-input', '参考图片', '', 180, 180),
  ]
  const edges: WorkflowEdge[] = [
    {
      id: 'sprite-input',
      source: 'sprite-trigger',
      target: 'sprite-source',
      sourcePort: 'output',
      targetPort: 'input',
    },
  ]
  const actions = [
    {
      id: 'idle',
      label: '待机呼吸',
      prompt: 'Subtle breathing idle cycle. Preserve the reference style; the feet stay planted.',
    },
    {
      id: 'walk',
      label: '走路',
      prompt:
        'Walking cycle: alternating foot contact, passing pose and opposite contact. Keep the character in place.',
    },
    {
      id: 'run',
      label: '跑步',
      prompt:
        'Running cycle: contact, compression, passing and flight. Preserve proportions and keep the character in place.',
    },
    {
      id: 'attack',
      label: '攻击',
      prompt:
        'Attack sequence: anticipation, wind-up, strike and recovery. Use only the equipment visible in the reference.',
    },
  ]
  actions.forEach((action, index) => {
    const y = index * 140
    const chain = [
      createWorkflowNode(
        `${action.id}-generate`,
        'media-generate',
        action.label,
        `${action.prompt}\n{{inputs.task}}`,
        380,
        y,
        {
          image: normalizeWorkflowImageSettings({
            action: action.id,
            frameCount: 4,
            directions: [...WORKFLOW_IMAGE_DIRECTIONS],
          }),
        },
      ),
      createWorkflowNode(`${action.id}-background`, 'media-background', '色键去背景', '', 560, y),
      createWorkflowNode(`${action.id}-frames`, 'media-frames', '拆分连续帧', '', 740, y),
      createWorkflowNode(`${action.id}-align`, 'media-transform', '裁边与脚底对齐', '', 920, y),
    ]
    nodes.push(...chain)
    const ids = ['sprite-source', ...chain.map((node) => node.id), 'sprite-preview']
    for (let i = 1; i < ids.length; i++)
      edges.push({
        id: `${action.id}-edge-${i}`,
        source: ids[i - 1],
        target: ids[i],
        sourcePort: 'output',
        targetPort: 'input',
      })
  })
  nodes.push(
    createWorkflowNode('sprite-preview', 'media-preview', '动画预览', '', 1100, 180),
    createWorkflowNode('sprite-export', 'media-export', '输出精灵图集', '', 1280, 180),
  )
  edges.push({
    id: 'sprite-output',
    source: 'sprite-preview',
    target: 'sprite-export',
    sourcePort: 'output',
    targetPort: 'input',
  })
  return {
    id: 'sprite',
    name: '游戏精灵图生成',
    description: '参考图 → 分方向动作 → 去背景 → 拆帧对齐 → 动画预览 → 图集',
    Icon: Image,
    nodes,
    edges,
    inputs: [
      {
        id: 'sprite-reference',
        name: 'reference',
        type: 'image',
        required: true,
        label: '角色参考图',
        description: '上传图片并保留原图风格。',
        defaultValue: null,
      },
      {
        id: 'sprite-task',
        name: 'task',
        type: 'text',
        required: false,
        label: '角色与动作要求',
        description: '本次运行的角色特征、动作及其他要求。',
        defaultValue: '',
      },
    ],
  }
}

WORKFLOW_TEMPLATES.push(spriteWorkflowTemplate())

export function workflowImageRequestCount(workflow: Pick<Workflow, 'nodes'>) {
  return workflow.nodes.reduce(
    (count, node) =>
      count +
      (node.enabled && node.kind === 'media-generate'
        ? normalizeWorkflowImageSettings(node.image).directions.length
        : 0),
    0,
  )
}

export function blankWorkflow(cwd = '', t?: WorkflowTranslate): Workflow {
  const nodes = [
    createWorkflowNode(crypto.randomUUID(), 'trigger', '手动触发', '', 65, 45),
    createWorkflowNode(
      crypto.randomUUID(),
      'prompt',
      '运行 Prompt',
      reusablePrompt('按本次输入完成任务，结合材料执行所需步骤，验证产物并报告结果。'),
      235,
      45,
    ),
  ]
  return {
    id: '',
    name: '未命名工作流',
    description: '',
    status: 'draft',
    revision: 1,
    cwd,
    model: null,
    inputs: reusableInputs(t),
    tags: [],
    visibility: 'private',
    notifications: [],
    nodes,
    edges: linearEdges(nodes),
  }
}

function spriteNodeLabel(node: WorkflowNode, t: WorkflowTranslate) {
  if (node.kind === 'media-generate') {
    if (node.image?.action === 'idle') return t('workflows:imageNodes.idle')
    if (node.image?.action === 'walk') return t('workflows:imageNodes.walk')
    if (node.image?.action === 'run') return t('workflows:imageNodes.run')
    if (node.image?.action === 'attack') return t('workflows:imageNodes.attack')
  }
  return paletteLabel(node.kind, t)
}

export function templateWorkflow(
  template: WorkflowTemplate,
  cwd = '',
  t?: WorkflowTranslate,
): Workflow {
  const nodes = template.nodes.map((item) => ({
    ...structuredClone(item),
    ...(template.id === 'sprite' && t ? { label: spriteNodeLabel(item, t) } : {}),
    id: crypto.randomUUID(),
  }))
  const labels = WORKFLOW_TEMPLATES.includes(template) ? reusableInputs(t) : []
  const nodeIds = new Map(template.nodes.map((node, index) => [node.id, nodes[index].id]))
  return {
    ...blankWorkflow(cwd, t),
    name: t && WORKFLOW_TEMPLATES.includes(template) ? templateName(template.id, t) : template.name,
    description:
      t && WORKFLOW_TEMPLATES.includes(template)
        ? templateDescription(template.id, t)
        : template.description,
    inputs: template.inputs.map((input) => {
      const localized = labels.find((item) => item.name === input.name)
      return {
        ...structuredClone(input),
        ...(localized ? { label: localized.label, description: localized.description } : {}),
        ...(template.id === 'sprite' && input.name === 'reference' && t
          ? {
              label: t('workflows:imageNodes.reference'),
              description: t('workflows:imageNodes.referenceHint'),
            }
          : {}),
        id: crypto.randomUUID(),
      }
    }),
    nodes,
    edges: template.edges
      ? template.edges.map((edge) => ({
          ...edge,
          id: crypto.randomUUID(),
          source: nodeIds.get(edge.source) ?? '',
          target: nodeIds.get(edge.target) ?? '',
        }))
      : linearEdges(nodes),
  }
}
