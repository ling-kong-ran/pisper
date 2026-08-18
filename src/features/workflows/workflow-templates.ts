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
import { createLinearWorkflowEdges } from '@shared/workflow-graph.mjs'
import type { I18nValues } from '@/app/i18n'
import type { LucideIcon } from 'lucide-react'
import type { NodeKind, Workflow, WorkflowNode } from './types'

export type WorkflowTranslate = (message: string, values?: I18nValues) => string
export type WorkflowTemplate = {
  id: string
  name: string
  description: string
  Icon: LucideIcon
  nodes: WorkflowNode[]
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
  if (templateId === 'pr-fix') return t('workflows:workflowsPage.prFix')
  if (templateId === 'research') return t('workflows:workflowsPage.research')
  if (templateId === 'report') return t('workflows:workflowsPage.dailyWeeklyReport')
  if (templateId === 'asset') return t('workflows:workflowsPage.assetGeneration')
  if (templateId === 'release') return t('workflows:workflowsPage.releasePreparation')
  return t('workflows:workflowsPage.codeReview')
}

export function templateDescription(templateId: string, t: WorkflowTranslate) {
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
    ...extra,
  }
}

function linearEdges(nodes: WorkflowNode[]) {
  return createLinearWorkflowEdges(nodes, () => crypto.randomUUID())
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
]

export function blankWorkflow(cwd = ''): Workflow {
  const nodes = [
    createWorkflowNode(crypto.randomUUID(), 'trigger', '手动触发', '', 65, 45),
    createWorkflowNode(crypto.randomUUID(), 'prompt', '运行 Prompt', '', 235, 45),
  ]
  return {
    id: '',
    name: '未命名工作流',
    description: '',
    status: 'draft',
    revision: 1,
    cwd,
    model: null,
    inputs: [],
    tags: [],
    visibility: 'private',
    notifications: [],
    nodes,
    edges: linearEdges(nodes),
  }
}

export function templateWorkflow(template: WorkflowTemplate, cwd = ''): Workflow {
  const nodes = template.nodes.map((item) => ({ ...item, id: crypto.randomUUID() }))
  return {
    ...blankWorkflow(cwd),
    name: template.name,
    description: template.description,
    nodes,
    edges: linearEdges(nodes),
  }
}
