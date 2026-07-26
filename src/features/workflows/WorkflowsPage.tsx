import { useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'
import {
  AlertTriangle,
  Bell,
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Code2,
  Copy,
  File,
  FileCode2,
  GitBranch,
  GripVertical,
  Image,
  MessageCircle,
  Pencil,
  Play,
  RefreshCw,
  Rocket,
  Search,
  Server,
  Square,
  Trash2,
  Zap,
} from 'lucide-react'
import { workflowPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { Panel, SectionTitle, Segmented, Toggle } from '@/components/ui'
import { AppSelect } from '@/components/AppSelect'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { apiJson } from '@/lib/api'
import { relativeTime } from '@/lib/format'
import { createLinearWorkflowEdges, wouldCreateWorkflowCycle } from '@shared/workflow-graph.mjs'
import { WorkflowCanvas, WorkflowPreview } from './WorkflowCanvas'
import type { NodeKind, WorkflowEdge, WorkflowNode } from './types'
import type { LucideIcon } from 'lucide-react'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

type NotificationTarget = 'browser' | 'feishu' | 'weixin'
type Workflow = {
  id: string
  name: string
  description: string
  status: 'draft' | 'published'
  cwd: string
  model: { provider: string; model: string } | null
  notifications: NotificationTarget[]
  nodes: WorkflowNode[]
  edges: WorkflowEdge[]
  lastRunAt?: string | null
  lastStatus?: string
}
type WorkflowRun = {
  id: string
  workflowId: string
  status: 'running' | 'completed' | 'failed' | 'cancelled'
  startedAt: string
  durationMs?: number
  completedNodes?: number
  totalNodes?: number
  currentNodeLabel?: string
  summary?: string
  error?: string
}
type WorkflowModel = { provider: string; model: string; label: string }
type NotificationTargets = Record<NotificationTarget, { enabled: boolean }>
type WorkflowsData = {
  workflows: Workflow[]
  runs: WorkflowRun[]
  limits: { maxConcurrent: number; running: number }
  notificationTargets: NotificationTargets
  models: WorkflowModel[]
  cwd: string
}
type WorkflowMutationResult = { workflow: Workflow; state: WorkflowsData }
type WorkflowTemplate = {
  id: string
  name: string
  description: string
  Icon: LucideIcon
  nodes: WorkflowNode[]
}
type WorkflowsPageProps = {
  notify: Notify
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
  query?: string
}
type WorkflowBuilderProps = {
  notify: Notify
  registerPrimaryAction: (action: () => void | Promise<unknown>) => () => void
  registerWorkflowActions?: (actions: {
    save: () => void | Promise<unknown>
    run: () => void | Promise<unknown>
    busy: boolean
    running: boolean
  }) => () => void
}
type Translate = ReturnType<typeof useI18n>['t']
type WorkflowFilter = 'all' | 'presets' | 'custom' | 'running' | 'failed' | 'draft'

const WORKFLOW_FILTERS: WorkflowFilter[] = [
  'all',
  'presets',
  'custom',
  'running',
  'failed',
  'draft',
]

const NODE_TYPES: Record<NodeKind, string> = {
  trigger: '触发器',
  prompt: '任务',
  file: '文件',
  mcp: 'MCP',
  notification: '通知',
  condition: '判断',
  parallel: '并行',
  approval: '审批',
}

const PALETTE = [
  { kind: 'trigger', label: '手动触发', Icon: Zap },
  { kind: 'prompt', label: '运行 Prompt', Icon: Bot },
  { kind: 'file', label: '读写文件', Icon: FileCode2 },
  { kind: 'mcp', label: '调用 MCP', Icon: Server },
  { kind: 'notification', label: '发送通知', Icon: Bell },
]

const TARGETS = {
  browser: { name: '通知', Icon: Bell },
  feishu: { name: '飞书', Icon: Bot },
  weixin: { name: '微信', Icon: MessageCircle },
}

function workflowFilterLabel(filter: WorkflowFilter, t: Translate) {
  if (filter === 'presets') return t('workflows:workflowsPage.presets')
  if (filter === 'custom') return t('workflows:workflowsPage.custom')
  if (filter === 'running') return t('workflows:workflowsPage.running')
  if (filter === 'failed') return t('workflows:workflowsPage.failed')
  if (filter === 'draft') return t('workflows:workflowsPage.draft')
  return t('workflows:workflowsPage.all')
}

function nodeTypeLabel(kind: NodeKind, t: Translate) {
  if (kind === 'trigger') return t('workflows:workflowsPage.triggerNode')
  if (kind === 'file') return t('workflows:workflowsPage.fileNode')
  if (kind === 'mcp') return t('workflows:workflowsPage.mcpNode')
  if (kind === 'notification') return t('workflows:workflowsPage.notificationNode')
  if (kind === 'condition') return t('workflows:workflowsPage.conditionNode')
  if (kind === 'parallel') return t('workflows:workflowsPage.parallelNode')
  if (kind === 'approval') return t('workflows:workflowsPage.approvalNode')
  return t('workflows:workflowsPage.task')
}

function paletteLabel(kind: NodeKind, t: Translate) {
  if (kind === 'trigger') return t('workflows:workflowsPage.manualTrigger')
  if (kind === 'file') return t('workflows:workflowsPage.readWriteFiles')
  if (kind === 'mcp') return t('workflows:workflowsPage.callMcp')
  if (kind === 'notification') return t('workflows:workflowsPage.sendNotification')
  return t('workflows:workflowsPage.runPrompt')
}

function notificationTargetLabel(target: NotificationTarget, t: Translate) {
  if (target === 'feishu') return t('workflows:workflowsPage.feishu')
  if (target === 'weixin') return t('workflows:workflowsPage.weChat')
  return t('workflows:workflowsPage.browserNotification')
}

function templateName(templateId: string, t: Translate) {
  if (templateId === 'pr-fix') return t('workflows:workflowsPage.prFix')
  if (templateId === 'research') return t('workflows:workflowsPage.research')
  if (templateId === 'report') return t('workflows:workflowsPage.dailyWeeklyReport')
  if (templateId === 'asset') return t('workflows:workflowsPage.assetGeneration')
  if (templateId === 'release') return t('workflows:workflowsPage.releasePreparation')
  return t('workflows:workflowsPage.codeReview')
}

function templateDescription(templateId: string, t: Translate) {
  if (templateId === 'pr-fix') return t('workflows:workflowsPage.prFixDescription')
  if (templateId === 'research') return t('workflows:workflowsPage.researchDescription')
  if (templateId === 'report') return t('workflows:workflowsPage.reportDescription')
  if (templateId === 'asset') return t('workflows:workflowsPage.assetDescription')
  if (templateId === 'release') return t('workflows:workflowsPage.releaseDescription')
  return t('workflows:workflowsPage.codeReviewDescription')
}

function node(
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
    retries: 0,
    timeoutMinutes: 20,
    failurePolicy: 'stop',
    enabled: true,
    ...extra,
  }
}

function linearEdges(nodes: WorkflowNode[]): WorkflowEdge[] {
  return createLinearWorkflowEdges(nodes, () => crypto.randomUUID())
}

const TEMPLATES: WorkflowTemplate[] = [
  {
    id: 'code-review',
    name: '代码审查',
    description: '读取 diff → 运行测试 → 生成 review',
    Icon: Code2,
    nodes: [
      node('review-trigger', 'trigger', '手动触发', '', 65, 45),
      node(
        'review-diff',
        'file',
        '读取 diff',
        '读取当前工作区的 git diff，识别改动范围与高风险文件。',
        235,
        45,
      ),
      node(
        'review-test',
        'prompt',
        '运行检查',
        '运行适合当前项目的测试与 lint，记录失败原因。',
        405,
        45,
      ),
      node(
        'review-report',
        'prompt',
        '生成 review',
        '结合 diff 和验证结果，输出按严重度排序的代码审查结论。',
        235,
        180,
      ),
      node('review-notify', 'notification', '发送结果', '', 405, 180),
    ],
  },
  {
    id: 'pr-fix',
    name: 'PR 修复',
    description: '定位失败 → 修改代码 → 回归测试',
    Icon: GitBranch,
    nodes: [
      node('fix-trigger', 'trigger', '手动触发', '', 65, 45),
      node('fix-find', 'prompt', '定位失败', '检查项目状态与失败信息，定位最可能的根因。', 235, 45),
      node(
        'fix-code',
        'prompt',
        '修改代码',
        '修复已定位的问题，保留用户已有改动，不执行破坏性命令。',
        405,
        45,
      ),
      node(
        'fix-test',
        'prompt',
        '回归测试',
        '运行针对性测试和构建，确认修复没有引入回归。',
        235,
        180,
      ),
      node('fix-notify', 'notification', '通知结果', '', 405, 180),
    ],
  },
  {
    id: 'research',
    name: '资料调研',
    description: '搜索资料 → 提取引用 → 点亮星忆',
    Icon: Search,
    nodes: [
      node('research-trigger', 'trigger', '手动输入', '', 65, 45),
      node(
        'research-search',
        'prompt',
        '搜索资料',
        '围绕工作流描述中的主题检索项目内资料与可用信息源。',
        235,
        45,
      ),
      node(
        'research-summary',
        'prompt',
        '整理引用',
        '整理关键结论、证据、限制和下一步建议。',
        405,
        45,
      ),
      node(
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
      node('report-trigger', 'trigger', '手动触发', '', 65, 45),
      node(
        'report-collect',
        'prompt',
        '汇总进展',
        '汇总当前项目近期完成事项、风险与待办。',
        235,
        45,
      ),
      node('report-write', 'prompt', '生成报告', '将汇总内容整理为清晰的日报或周报。', 405, 45),
      node('report-notify', 'notification', '渠道通知', '', 320, 180),
    ],
  },
  {
    id: 'asset',
    name: '资产生成',
    description: '生成图片 → 存入资产库 → 通知验收',
    Icon: Image,
    nodes: [
      node('asset-trigger', 'trigger', '手动输入', '', 65, 45),
      node(
        'asset-generate',
        'prompt',
        '生成视觉资产',
        '根据工作流描述生成需要的视觉资产，并保存生成文件。',
        235,
        45,
      ),
      node(
        'asset-check',
        'prompt',
        '检查产物',
        '检查生成资产是否完整、可访问并符合需求。',
        405,
        45,
      ),
      node('asset-notify', 'notification', '通知验收', '', 320, 180),
    ],
  },
  {
    id: 'release',
    name: '发布准备',
    description: '版本检查 → changelog → 创建发布单',
    Icon: Rocket,
    nodes: [
      node('release-trigger', 'trigger', '手动触发', '', 65, 45),
      node(
        'release-check',
        'prompt',
        '版本检查',
        '检查工作区、测试、构建和版本信息是否满足发布要求。',
        235,
        45,
      ),
      node(
        'release-log',
        'prompt',
        '生成 changelog',
        '根据近期提交和改动生成 changelog 与发布说明。',
        405,
        45,
      ),
      node('release-report', 'prompt', '发布清单', '生成最终发布检查清单并标记阻塞项。', 320, 180),
    ],
  },
]

function blankWorkflow(cwd = ''): Workflow {
  const nodes = [
    node(crypto.randomUUID(), 'trigger', '手动触发', '', 65, 45),
    node(crypto.randomUUID(), 'prompt', '运行 Prompt', '', 235, 45),
  ]
  return {
    id: '',
    name: '未命名工作流',
    description: '',
    status: 'draft',
    cwd,
    model: null,
    notifications: [],
    nodes,
    edges: linearEdges(nodes),
  }
}

function templateWorkflow(template: WorkflowTemplate, cwd = ''): Workflow {
  const nodes = template.nodes.map((item) => ({ ...item, id: crypto.randomUUID() }))
  return {
    ...blankWorkflow(cwd),
    name: template.name,
    description: template.description,
    nodes,
    edges: linearEdges(nodes),
  }
}

function runProgress(run?: WorkflowRun) {
  if (!run) return 0
  if (run.status === 'completed') return 100
  return Math.round(
    ((Number(run.completedNodes) || 0) / Math.max(1, Number(run.totalNodes) || 1)) * 100,
  )
}

function runTone(status?: WorkflowRun['status']) {
  return status === 'completed'
    ? 'green'
    : status === 'failed'
      ? 'amber'
      : status === 'cancelled'
        ? 'blue'
        : 'blue'
}

function durationLabel(durationMs?: number) {
  const seconds = Math.max(0, Math.round((Number(durationMs) || 0) / 1000))
  return seconds < 60 ? `${seconds} 秒` : `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`
}

function errorMessage(caught: unknown) {
  return caught instanceof Error ? caught.message : String(caught)
}

const EMPTY_NOTIFICATION_TARGETS: NotificationTargets = {
  browser: { enabled: false },
  feishu: { enabled: false },
  weixin: { enabled: false },
}

export function WorkflowsPage({ notify, requestConfirm, query = '' }: WorkflowsPageProps) {
  const { t, language } = useI18n()
  const routerNavigate = useNavigate()
  const [data, setData] = useState<WorkflowsData>({
    workflows: [],
    runs: [],
    limits: { maxConcurrent: 4, running: 0 },
    notificationTargets: EMPTY_NOTIFICATION_TARGETS,
    models: [],
    cwd: '',
  })
  const [filter, setFilter] = useState<WorkflowFilter>('all')
  const [loading, setLoading] = useState(true)
  const [busyId, setBusyId] = useState('')
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      setData(await apiJson<WorkflowsData>('/api/workflows'))
      setError('')
    } catch (caught) {
      setError(errorMessage(caught))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])
  useEffect(() => {
    const timer = window.setInterval(
      () => {
        void load()
      },
      data.limits?.running ? 1500 : 8000,
    )
    return () => window.clearInterval(timer)
  }, [data.limits?.running, load])

  const latestRun = useCallback(
    (workflowId: string) =>
      data.runs
        .filter((run) => run.workflowId === workflowId)
        .sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt))[0],
    [data.runs],
  )
  const visible = useMemo(
    () =>
      data.workflows.filter((workflow) => {
        const run = latestRun(workflow.id)
        const matchesQuery = `${workflow.name} ${workflow.description}`
          .toLowerCase()
          .includes(query.toLowerCase())
        if (!matchesQuery) return false
        if (filter === 'running') return run?.status === 'running'
        if (filter === 'failed') return run?.status === 'failed'
        if (filter === 'draft') return workflow.status === 'draft'
        if (filter === 'presets') return false
        return true
      }),
    [data.workflows, filter, latestRun, query],
  )

  const openTemplate = (templateId: string) =>
    routerNavigate(`${workflowPath('new')}?template=${encodeURIComponent(templateId)}`)
  const openWorkflow = (workflowId: string) => routerNavigate(workflowPath(workflowId))

  const runWorkflow = async (workflow: Workflow) => {
    setBusyId(workflow.id)
    setError('')
    try {
      await apiJson(`/api/workflows/${encodeURIComponent(workflow.id)}/run`, {
        method: 'POST',
        body: '{}',
      })
      await load()
      notify(t('workflows:workflowsPage.workflowStarted'))
    } catch (caught) {
      const message = errorMessage(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusyId('')
    }
  }

  const stopRun = async (run: WorkflowRun) => {
    setBusyId(run.id)
    setError('')
    try {
      await apiJson(`/api/workflows/runs/${encodeURIComponent(run.id)}/stop`, {
        method: 'POST',
        body: '{}',
      })
      await load()
      notify(t('workflows:workflowsPage.stoppingWorkflow'), 'info')
    } catch (caught) {
      const message = errorMessage(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusyId('')
    }
  }

  const removeWorkflow = async (workflow: Workflow) => {
    const approved = await requestConfirm?.({
      title: t('workflows:workflowsPage.deleteWorkflow'),
      message: t('workflows:workflowsPage.deleteWorkflowNameAndItsRunHistory', {
        name: workflow.name,
      }),
      confirmLabel: t('workflows:workflowsPage.delete'),
      tone: 'danger',
    })
    if (!approved) return
    setBusyId(workflow.id)
    try {
      await apiJson(`/api/workflows/${encodeURIComponent(workflow.id)}`, { method: 'DELETE' })
      await load()
      notify(t('workflows:workflowsPage.workflowDeleted'))
    } catch (caught) {
      const message = errorMessage(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusyId('')
    }
  }

  const preview = data.workflows[0]
  const published = data.workflows.filter((workflow) => workflow.status === 'published').length
  const notificationCount = Object.values(data.notificationTargets || {}).filter(
    (target) => target.enabled,
  ).length

  if (loading)
    return (
      <Panel className="empty-state">
        <RefreshCw className="spin" size={23} />
        <h2>{t('workflows:workflowsPage.loadingWorkflows')}</h2>
      </Panel>
    )
  return (
    <div className="workflows-page">
      {error && (
        <div className="config-error">
          <AlertTriangle size={13} />
          {error}
        </div>
      )}
      <Segmented
        options={WORKFLOW_FILTERS.map((item) => workflowFilterLabel(item, t))}
        value={workflowFilterLabel(filter, t)}
        onChange={(label) =>
          setFilter(
            WORKFLOW_FILTERS.find((item) => workflowFilterLabel(item, t) === label) || 'all',
          )
        }
      />
      <div className="workflow-top">
        <Panel>
          <div className="card-head">
            <SectionTitle title={t('workflows:workflowsPage.commonTemplates')} />
            <a>{t('workflows:workflowsPage.countTemplates', { count: TEMPLATES.length })}</a>
          </div>
          <div className="template-grid">
            {TEMPLATES.map((template) => {
              const Icon = template.Icon
              return (
                <button onClick={() => openTemplate(template.id)} key={template.id}>
                  <span className="list-icon">
                    <Icon size={15} />
                  </span>
                  <span>
                    <strong>{templateName(template.id, t)}</strong>
                    <small>{templateDescription(template.id, t)}</small>
                  </span>
                  <ChevronRight size={14} />
                </button>
              )
            })}
          </div>
        </Panel>
        <Panel className="workflow-preview">
          <div className="card-head">
            <div>
              <SectionTitle title={t('workflows:workflowsPage.customWorkflow')} />
              {preview && (
                <small>
                  {preview.name} ·{' '}
                  {preview.status === 'published'
                    ? t('workflows:workflowsPage.published')
                    : t('workflows:workflowsPage.draft')}
                </small>
              )}
            </div>
            <button className="text-button" onClick={() => routerNavigate(workflowPath('new'))}>
              {t('workflows:workflowsPage.startBlank')}
            </button>
          </div>
          {preview?.nodes.length ? (
            <WorkflowPreview
              nodes={preview.nodes}
              edges={preview.edges || []}
              nodeTypeLabel={(kind) => nodeTypeLabel(kind, t)}
            />
          ) : (
            <div className="channel-route-empty compact">
              <strong>{t('workflows:workflowsPage.noCustomWorkflowYet')}</strong>
            </div>
          )}
        </Panel>
      </div>
      <div className="workflow-bottom">
        <Panel>
          <div className="card-head">
            <SectionTitle title={t('workflows:workflowsPage.workflows')} />
            <a>{t('workflows:workflowsPage.countWorkflows', { count: visible.length })}</a>
          </div>
          {visible.length ? (
            visible.map((workflow) => {
              const run = latestRun(workflow.id)
              const progress = runProgress(run)
              const running = run?.status === 'running'
              return (
                <div className="run-row" key={workflow.id}>
                  <span>
                    <strong>{workflow.name}</strong>
                    <small>
                      {running
                        ? t('workflows:workflowsPage.runningNode', {
                            node: run.currentNodeLabel || t('workflows:workflowsPage.preparing'),
                          })
                        : workflow.lastRunAt
                          ? `${workflow.lastStatus === 'completed' ? t('workflows:workflowsPage.completed') : workflow.lastStatus === 'failed' ? t('workflows:workflowsPage.failed') : workflow.lastStatus === 'cancelled' ? t('workflows:workflowsPage.stopped') : t('workflows:workflowsPage.draft')} · ${relativeTime(workflow.lastRunAt, language)}`
                          : workflow.status === 'published'
                            ? t('workflows:workflowsPage.published')
                            : t('workflows:workflowsPage.draft')}
                    </small>
                  </span>
                  <div className="run-progress">
                    <i className={runTone(run?.status)} style={{ width: `${progress}%` }} />
                  </div>
                  <em>{progress}%</em>
                  <div className="button-row">
                    {running ? (
                      <button disabled={busyId === run.id} onClick={() => void stopRun(run)}>
                        <Square size={12} />
                        {t('workflows:workflowsPage.stop')}
                      </button>
                    ) : (
                      <button
                        disabled={busyId === workflow.id}
                        onClick={() => void runWorkflow(workflow)}
                      >
                        <Play size={12} />
                        {t('workflows:workflowsPage.run')}
                      </button>
                    )}
                    <button onClick={() => openWorkflow(workflow.id)}>
                      <Pencil size={12} />
                      {t('workflows:workflowsPage.edit')}
                    </button>
                    <button
                      disabled={running || busyId === workflow.id}
                      onClick={() => void removeWorkflow(workflow)}
                    >
                      <Trash2 size={12} />
                      {t('workflows:workflowsPage.delete')}
                    </button>
                  </div>
                </div>
              )
            })
          ) : (
            <div className="channel-route-empty compact">
              <strong>
                {filter === 'presets'
                  ? t('workflows:workflowsPage.chooseATemplateAboveToGetStarted')
                  : t('workflows:workflowsPage.noMatchingWorkflows')}
              </strong>
            </div>
          )}
        </Panel>
        <Panel>
          <SectionTitle title={t('workflows:workflowsPage.queueAndLimits')} />
          {[
            [
              t('workflows:workflowsPage.maximumConcurrency'),
              String(data.limits?.maxConcurrent || 4),
              t('workflows:workflowsPage.countCurrentlyRunning', {
                count: data.limits?.running || 0,
              }),
            ],
            [
              t('workflows:workflowsPage.published'),
              String(published),
              t('workflows:workflowsPage.countWorkflowsTotal', { count: data.workflows.length }),
            ],
            [
              t('workflows:workflowsPage.retryOnFailure'),
              t('workflows:workflowsPage.upTo3TimesPerNode'),
              t('workflows:workflowsPage.configurablePerNode'),
            ],
            [
              t('workflows:workflowsPage.completionDelivery'),
              notificationCount
                ? t('workflows:workflowsPage.enabled')
                : t('workflows:workflowsPage.notEnabled'),
              t('workflows:workflowsPage.countAvailableChannels', { count: notificationCount }),
            ],
          ].map((row) => (
            <div className="setting-row" key={row[0]}>
              <span>
                <strong>{row[0]}</strong>
                <small>{row[2]}</small>
              </span>
              <button>{row[1]}</button>
            </div>
          ))}
        </Panel>
      </div>
    </div>
  )
}

export function WorkflowBuilder({
  notify,
  registerPrimaryAction,
  registerWorkflowActions,
}: WorkflowBuilderProps) {
  const { t, language } = useI18n()
  const routerNavigate = useNavigate()
  const { workflowId = 'new' } = useParams()
  const [searchParams] = useSearchParams()
  const [catalog, setCatalog] = useState<WorkflowsData>({
    workflows: [],
    runs: [],
    limits: { maxConcurrent: 4, running: 0 },
    models: [],
    notificationTargets: EMPTY_NOTIFICATION_TARGETS,
    cwd: '',
  })
  const [draft, setDraft] = useState<Workflow | null>(null)
  const [selectedId, setSelectedId] = useState('')
  const [selectedEdgeId, setSelectedEdgeId] = useState('')
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      const result = await apiJson<WorkflowsData>('/api/workflows')
      setCatalog(result)
      const stored =
        workflowId !== 'new'
          ? result.workflows.find((workflow) => workflow.id === workflowId)
          : null
      const template = TEMPLATES.find((item) => item.id === searchParams.get('template'))
      const next = stored
        ? structuredClone(stored)
        : template
          ? templateWorkflow(template, result.cwd)
          : blankWorkflow(result.cwd)
      setDraft(next)
      setSelectedId((current) =>
        next.nodes.some((item) => item.id === current) ? current : next.nodes[0]?.id || '',
      )
      setSelectedEdgeId('')
      setError(
        stored || workflowId === 'new'
          ? ''
          : t('workflows:workflowsPage.workflowNotFoundABlankEditorWasOpened'),
      )
    } catch (caught) {
      setError(errorMessage(caught))
    } finally {
      setLoading(false)
    }
  }, [searchParams, t, workflowId])

  useEffect(() => {
    void load()
  }, [load])
  const currentRun = useMemo(
    () =>
      catalog.runs
        .filter((run) => run.workflowId === draft?.id)
        .sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt))[0],
    [catalog.runs, draft?.id],
  )
  const running = currentRun?.status === 'running'
  useEffect(() => {
    if (!running) return undefined
    const timer = window.setInterval(async () => {
      try {
        setCatalog(await apiJson<WorkflowsData>('/api/workflows'))
      } catch {}
    }, 1500)
    return () => window.clearInterval(timer)
  }, [running])

  const updateDraft = (patch: Partial<Workflow>) =>
    setDraft((current) => (current ? { ...current, ...patch } : current))
  const updateNode = (patch: Partial<WorkflowNode>) =>
    setDraft((current) =>
      current
        ? {
            ...current,
            nodes: current.nodes.map((item) =>
              item.id === selectedId ? { ...item, ...patch } : item,
            ),
          }
        : current,
    )
  const current = selectedId ? draft?.nodes.find((item) => item.id === selectedId) || null : null
  const selectedEdge = draft?.edges?.find((edge) => edge.id === selectedEdgeId) || null

  const addEdge = useCallback(
    (source: string, target: string, sourcePort = 'output') => {
      if (!draft || source === target) return
      const targetNode = draft.nodes.find((item) => item.id === target)
      if (!targetNode || targetNode.kind === 'trigger') {
        notify(t('workflows:workflowsPage.aTriggerCannotHaveAnUpstreamConnection'), 'error')
        return
      }
      if (
        (draft.edges || []).some(
          (edge) =>
            edge.source === source && edge.target === target && edge.sourcePort === sourcePort,
        )
      ) {
        notify(t('workflows:workflowsPage.thisConnectionAlreadyExists'), 'info')
        return
      }
      if (wouldCreateWorkflowCycle(draft.nodes, draft.edges || [], source, target, sourcePort)) {
        notify(t('workflows:workflowsPage.aWorkflowCannotContainCyclicConnections'), 'error')
        return
      }
      const edge = { id: crypto.randomUUID(), source, sourcePort, target, targetPort: 'input' }
      setDraft((currentDraft) =>
        currentDraft
          ? { ...currentDraft, edges: [...(currentDraft.edges || []), edge] }
          : currentDraft,
      )
      setSelectedEdgeId(edge.id)
      setSelectedId('')
      notify(t('workflows:workflowsPage.connectionCreated'), 'info')
    },
    [draft, notify, t],
  )

  const removeEdges = useCallback(
    (edgeIds: string[]) => {
      if (!edgeIds.length) return
      const ids = new Set(edgeIds)
      setDraft((currentDraft) =>
        currentDraft
          ? {
              ...currentDraft,
              edges: (currentDraft.edges || []).filter((edge) => !ids.has(edge.id)),
            }
          : currentDraft,
      )
      setSelectedEdgeId('')
      notify(t('workflows:workflowsPage.connectionDeleted'), 'info')
    },
    [notify, t],
  )

  const removeSelectedEdge = useCallback(() => {
    if (!selectedEdgeId) return
    removeEdges([selectedEdgeId])
  }, [removeEdges, selectedEdgeId])

  const removeNodes = useCallback(
    (nodeIds: string[]) => {
      if (!nodeIds.length) return
      const ids = new Set(nodeIds)
      setDraft((currentDraft) =>
        currentDraft
          ? {
              ...currentDraft,
              nodes: currentDraft.nodes.filter((item) => !ids.has(item.id)),
              edges: (currentDraft.edges || []).filter(
                (edge) => !ids.has(edge.source) && !ids.has(edge.target),
              ),
            }
          : currentDraft,
      )
      setSelectedId('')
      setSelectedEdgeId('')
      notify(t('workflows:workflowsPage.nodeDeleted'), 'info')
    },
    [notify, t],
  )

  const saveWorkflow = useCallback(
    async (status: Workflow['status'] = 'draft', quiet = false) => {
      if (!draft) return null
      setBusy(true)
      setError('')
      try {
        const payload = { ...draft, status }
        const result = draft.id
          ? await apiJson<WorkflowMutationResult>(
              `/api/workflows/${encodeURIComponent(draft.id)}`,
              {
                method: 'PATCH',
                body: JSON.stringify(payload),
              },
            )
          : await apiJson<WorkflowMutationResult>('/api/workflows', {
              method: 'POST',
              body: JSON.stringify(payload),
            })
        setCatalog(result.state)
        setDraft(structuredClone(result.workflow))
        if (!draft.id) routerNavigate(workflowPath(result.workflow.id), { replace: true })
        if (!quiet)
          notify(
            status === 'published'
              ? t('workflows:workflowsPage.workflowPublished')
              : t('workflows:workflowsPage.workflowDraftSaved'),
          )
        return result.workflow
      } catch (caught) {
        const message = errorMessage(caught)
        setError(message)
        notify(message, 'error')
        return null
      } finally {
        setBusy(false)
      }
    },
    [draft, notify, routerNavigate, t],
  )

  const runWorkflow = useCallback(async () => {
    const workflow = await saveWorkflow(draft?.status || 'draft', true)
    if (!workflow) return
    setBusy(true)
    try {
      await apiJson(`/api/workflows/${encodeURIComponent(workflow.id)}/run`, {
        method: 'POST',
        body: '{}',
      })
      setCatalog(await apiJson<WorkflowsData>('/api/workflows'))
      notify(t('workflows:workflowsPage.workflowStarted'))
    } catch (caught) {
      const message = errorMessage(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }, [draft?.status, notify, saveWorkflow, t])

  const stopWorkflow = useCallback(async () => {
    if (!currentRun || currentRun.status !== 'running') return
    setBusy(true)
    try {
      await apiJson(`/api/workflows/runs/${encodeURIComponent(currentRun.id)}/stop`, {
        method: 'POST',
        body: '{}',
      })
      setCatalog(await apiJson<WorkflowsData>('/api/workflows'))
      notify(t('workflows:workflowsPage.stoppingWorkflow'), 'info')
    } catch (caught) {
      const message = errorMessage(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }, [currentRun, notify, t])

  const publish = useCallback(() => saveWorkflow('published'), [saveWorkflow])
  usePagePrimaryAction(registerPrimaryAction, publish)
  useEffect(
    () =>
      registerWorkflowActions?.({
        save: () => saveWorkflow('draft'),
        run: running ? stopWorkflow : runWorkflow,
        busy,
        running,
      }),
    [busy, registerWorkflowActions, runWorkflow, running, saveWorkflow, stopWorkflow],
  )

  const addNode = useCallback(
    (kind: NodeKind, label: string, position: { x: number; y: number }) => {
      const id = crypto.randomUUID()
      setDraft((currentDraft) =>
        currentDraft
          ? {
              ...currentDraft,
              nodes: [
                ...currentDraft.nodes,
                node(id, kind, label || NODE_TYPES[kind], '', position.x, position.y),
              ],
            }
          : currentDraft,
      )
      setSelectedId(id)
      setSelectedEdgeId('')
    },
    [],
  )

  const moveNode = useCallback((id: string, position: { x: number; y: number }) => {
    setDraft((currentDraft) =>
      currentDraft
        ? {
            ...currentDraft,
            nodes: currentDraft.nodes.map((item) =>
              item.id === id && (item.x !== position.x || item.y !== position.y)
                ? { ...item, x: position.x, y: position.y }
                : item,
            ),
          }
        : currentDraft,
    )
  }, [])

  const copyNode = () => {
    if (!current || !draft) return
    const id = crypto.randomUUID()
    updateDraft({
      nodes: [
        ...draft.nodes,
        { ...current, id, label: `${current.label} 副本`, x: current.x + 25, y: current.y + 25 },
      ],
    })
    setSelectedId(id)
    setSelectedEdgeId('')
    notify(t('workflows:workflowsPage.nodeDuplicated'), 'info')
  }

  const deleteNode = () => {
    if (!current) return
    removeNodes([current.id])
  }

  const toggleNotification = (target: NotificationTarget) => {
    if (!draft) return
    updateDraft({
      notifications: draft.notifications.includes(target)
        ? draft.notifications.filter((item) => item !== target)
        : [...draft.notifications, target],
    })
  }
  const nodesById = new Map((draft?.nodes || []).map((item) => [item.id, item]))

  if (loading || !draft)
    return (
      <Panel className="empty-state">
        <RefreshCw className="spin" size={23} />
        <h2>{t('workflows:workflowsPage.loadingWorkflowEditor')}</h2>
      </Panel>
    )
  return (
    <div className="preview-page">
      {error && (
        <div className="config-error">
          <AlertTriangle size={13} />
          {error}
        </div>
      )}
      {running && (
        <div className="permission-note">
          <RefreshCw className="spin" size={16} />
          <span>
            <strong>{t('workflows:workflowsPage.workflowRunning')}</strong>
            <small>
              {t('workflows:workflowsPage.runningNodeCompletedTotalCompleted', {
                node: currentRun.currentNodeLabel || t('workflows:workflowsPage.preparing'),
                completed: currentRun.completedNodes,
                total: currentRun.totalNodes,
              })}
            </small>
          </span>
        </div>
      )}
      <div className="builder-layout">
        <Panel className="node-library">
          <SectionTitle title={t('workflows:workflowsPage.nodeLibrary')} />
          {PALETTE.map(({ kind, label, Icon }) => (
            <div key={kind}>
              <small>{kind === 'trigger' ? t('workflows:workflowsPage.triggerGroup') : ''}</small>
              <button
                draggable
                onDragStart={(event) =>
                  event.dataTransfer.setData('text/plain', JSON.stringify({ kind, label }))
                }
              >
                <Icon size={15} />
                {paletteLabel(kind as NodeKind, t)}
                <span
                  title={t('workflows:workflowsPage.drag')}
                  aria-label={t('workflows:workflowsPage.drag')}
                >
                  <GripVertical size={13} />
                </span>
              </button>
            </div>
          ))}
        </Panel>
        <Panel className="builder-canvas">
          <WorkflowCanvas
            nodes={draft.nodes}
            edges={draft.edges || []}
            selectedNodeId={selectedId}
            selectedEdgeId={selectedEdgeId}
            hint={t('workflows:workflowsPage.dragFromANodeOutputToTheTargetInputToConnectThem')}
            inputLabel={t('workflows:workflowsPage.inputPort')}
            outputLabel={t('workflows:workflowsPage.outputPort')}
            nodeTypeLabel={(kind) => nodeTypeLabel(kind, t)}
            onAddNode={addNode}
            onConnect={addEdge}
            onMoveNode={moveNode}
            onSelectNode={(id) => {
              setSelectedId(id)
              setSelectedEdgeId('')
            }}
            onSelectEdge={(id) => {
              setSelectedEdgeId(id)
              setSelectedId('')
            }}
            onClearSelection={() => {
              setSelectedId('')
              setSelectedEdgeId('')
            }}
            onDeleteNodes={removeNodes}
            onDeleteEdges={removeEdges}
          />
        </Panel>
        <div className="detail-stack inspector">
          <Panel>
            <SectionTitle title={t('workflows:workflowsPage.workflowSettings')} />
            <label className="field-label">
              {t('workflows:workflowsPage.name')}
              <input
                value={draft.name}
                onChange={(event) => updateDraft({ name: event.target.value })}
              />
            </label>
            <label className="field-label">
              {t('workflows:workflowsPage.description')}
              <textarea
                value={draft.description}
                onChange={(event) => updateDraft({ description: event.target.value })}
              />
            </label>
            <label className="field-label">
              {t('workflows:workflowsPage.workingDirectory')}
              <input
                value={draft.cwd}
                onChange={(event) => updateDraft({ cwd: event.target.value })}
              />
            </label>
            <label className="field-label">
              {t('workflows:workflowsPage.defaultModel')}
              <span className="select-wrap">
                <AppSelect
                  value={draft.model ? `${draft.model.provider}/${draft.model.model}` : ''}
                  onChange={(event) => {
                    const model = catalog.models.find(
                      (item) => `${item.provider}/${item.model}` === event.target.value,
                    )
                    updateDraft({
                      model: model ? { provider: model.provider, model: model.model } : null,
                    })
                  }}
                >
                  <option value="">{t('workflows:workflowsPage.useSystemDefault')}</option>
                  {catalog.models.map((model) => (
                    <option
                      value={`${model.provider}/${model.model}`}
                      key={`${model.provider}/${model.model}`}
                    >
                      {model.label}
                    </option>
                  ))}
                </AppSelect>
                <ChevronDown size={13} />
              </span>
            </label>
            {(
              Object.entries(TARGETS) as Array<
                [NotificationTarget, (typeof TARGETS)[NotificationTarget]]
              >
            ).map(([id, target]) => {
              const Icon = target.Icon
              return (
                <div className="toggle-line" key={id}>
                  <span>
                    <Icon size={15} />
                    {notificationTargetLabel(id, t)}
                  </span>
                  <Toggle
                    value={draft.notifications.includes(id)}
                    disabled={!catalog.notificationTargets[id]?.enabled}
                    onChange={() => toggleNotification(id)}
                  />
                </div>
              )
            })}
          </Panel>
          {selectedEdge && (
            <Panel>
              <SectionTitle title={t('workflows:workflowsPage.selectedConnection')} />
              <div className="workflow-edge-summary">
                <strong>
                  {nodesById.get(selectedEdge.source)?.label ||
                    t('workflows:workflowsPage.unknownNode')}
                </strong>
                <span>→</span>
                <strong>
                  {nodesById.get(selectedEdge.target)?.label ||
                    t('workflows:workflowsPage.unknownNode')}
                </strong>
              </div>
              <p className="muted-copy">
                {t('workflows:workflowsPage.pressDeleteOrBackspaceToRemoveThisConnection')}
              </p>
              <button className="button danger" onClick={removeSelectedEdge}>
                <Trash2 size={14} />
                {t('workflows:workflowsPage.deleteConnection')}
              </button>
            </Panel>
          )}
          <Panel>
            <SectionTitle title={t('workflows:workflowsPage.selectedNode')} />
            {current ? (
              <>
                <label className="field-label">
                  {t('workflows:workflowsPage.nodeName')}
                  <input
                    value={current.label}
                    onChange={(event) => updateNode({ label: event.target.value })}
                  />
                </label>
                <label className="field-label">
                  {t('workflows:workflowsPage.nodeModel')}
                  <span className="select-wrap">
                    <AppSelect
                      value={
                        current.model ? `${current.model.provider}/${current.model.model}` : ''
                      }
                      onChange={(event) => {
                        const model = catalog.models.find(
                          (item) => `${item.provider}/${item.model}` === event.target.value,
                        )
                        updateNode({
                          model: model ? { provider: model.provider, model: model.model } : null,
                        })
                      }}
                    >
                      <option value="">
                        {t('workflows:workflowsPage.inheritWorkflowDefaultModel')}
                      </option>
                      {catalog.models.map((model) => (
                        <option
                          value={`${model.provider}/${model.model}`}
                          key={`${model.provider}/${model.model}`}
                        >
                          {model.label}
                        </option>
                      ))}
                    </AppSelect>
                    <ChevronDown size={13} />
                  </span>
                </label>
                <div className="form-grid three">
                  <label className="field-label">
                    {t('workflows:workflowsPage.retryCount')}
                    <input
                      type="number"
                      min="0"
                      max="3"
                      value={current.retries}
                      onChange={(event) => updateNode({ retries: Number(event.target.value) })}
                    />
                  </label>
                  <label className="field-label">
                    {t('workflows:workflowsPage.timeoutMinutes')}
                    <input
                      type="number"
                      min="1"
                      max="240"
                      value={current.timeoutMinutes}
                      onChange={(event) =>
                        updateNode({ timeoutMinutes: Number(event.target.value) })
                      }
                    />
                  </label>
                  <label className="field-label">
                    {t('workflows:workflowsPage.failureHandling')}
                    <span className="select-wrap">
                      <AppSelect
                        value={current.failurePolicy}
                        onChange={(event) =>
                          updateNode({
                            failurePolicy: event.target.value === 'skip' ? 'skip' : 'stop',
                          })
                        }
                      >
                        <option value="stop">{t('workflows:workflowsPage.stopImmediately')}</option>
                        <option value="skip">{t('workflows:workflowsPage.skipThisNode')}</option>
                      </AppSelect>
                      <ChevronDown size={13} />
                    </span>
                  </label>
                </div>
                {['prompt', 'file', 'mcp', 'condition'].includes(current.kind) && (
                  <label className="field-label">
                    Prompt
                    <textarea
                      value={current.prompt}
                      onChange={(event) => updateNode({ prompt: event.target.value })}
                      placeholder={t(
                        'workflows:workflowsPage.describeTheWorkTheAgentShouldCompleteInThisNode',
                      )}
                    />
                  </label>
                )}
                <div className="button-row">
                  <button className="button secondary" onClick={copyNode}>
                    <Copy size={14} />
                    {t('workflows:workflowsPage.duplicateNode')}
                  </button>
                  <button className="button danger" onClick={deleteNode}>
                    <Trash2 size={14} />
                    {t('workflows:workflowsPage.deleteNode')}
                  </button>
                </div>
              </>
            ) : (
              <p className="muted-copy">
                {selectedEdge
                  ? t('workflows:workflowsPage.aConnectionIsCurrentlySelected')
                  : t('workflows:workflowsPage.dragNodesFromTheLeftToStartBuildingTheWorkflow')}
              </p>
            )}
          </Panel>
          {draft.id && (
            <Panel>
              <SectionTitle title={t('workflows:workflowsPage.latestRun')} />
              {currentRun ? (
                <div className={`activity-row ${currentRun.status}`}>
                  {currentRun.status === 'running' ? (
                    <RefreshCw className="spin" size={15} />
                  ) : currentRun.status === 'completed' ? (
                    <CheckCircle2 size={15} />
                  ) : (
                    <AlertTriangle size={15} />
                  )}
                  <span>
                    <strong>
                      {currentRun.status === 'completed'
                        ? currentRun.summary || t('workflows:workflowsPage.workflowCompleted')
                        : currentRun.status === 'running'
                          ? currentRun.currentNodeLabel || t('workflows:workflowsPage.running')
                          : currentRun.error || t('workflows:workflowsPage.workflowFailed')}
                    </strong>
                    <small>
                      {relativeTime(currentRun.startedAt, language)} ·{' '}
                      {durationLabel(currentRun.durationMs)}
                    </small>
                  </span>
                </div>
              ) : (
                <p className="muted-copy">{t('workflows:workflowsPage.noRunHistory')}</p>
              )}
            </Panel>
          )}
        </div>
      </div>
    </div>
  )
}
