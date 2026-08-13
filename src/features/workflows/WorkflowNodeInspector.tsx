import {
  AlertTriangle,
  Bell,
  Bot,
  ChevronDown,
  Copy,
  MessageCircle,
  Plus,
  Trash2,
} from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import type {
  NotificationTarget,
  Workflow,
  WorkflowEdge,
  WorkflowNode,
  WorkflowInputType,
  WorkflowRun,
  WorkflowsData,
} from './types'
import { WorkflowLatestRun } from './WorkflowRunControls'
import type { WorkflowTranslate } from './workflow-templates'

const NOTIFICATION_TARGETS = {
  browser: { Icon: Bell },
  feishu: { Icon: Bot },
  weixin: { Icon: MessageCircle },
}

function notificationTargetLabel(target: NotificationTarget, t: WorkflowTranslate) {
  if (target === 'feishu') return t('workflows:workflowsPage.feishu')
  if (target === 'weixin') return t('workflows:workflowsPage.weChat')
  return t('workflows:workflowsPage.browserNotification')
}

function WorkflowSettings({
  draft,
  catalog,
  t,
  onUpdateDraft,
  onToggleNotification,
  onOpenChannels,
}: {
  draft: Workflow
  catalog: WorkflowsData
  t: WorkflowTranslate
  onUpdateDraft: (patch: Partial<Workflow>) => void
  onToggleNotification: (target: NotificationTarget) => void
  onOpenChannels: () => void
}) {
  const hasNotificationTarget = Object.values(catalog.notificationTargets).some(
    (target) => target.enabled,
  )

  return (
    <Card size="sm" className="workflow-card gap-0 py-0">
      <CardContent className="p-3.5">
        <CardTitle className="workflow-section-title">
          {t('workflows:workflowsPage.workflowSettings')}
        </CardTitle>
        <label className="field-label">
          {t('workflows:workflowsPage.name')}
          <Input
            value={draft.name}
            onChange={(event) => onUpdateDraft({ name: event.target.value })}
          />
        </label>
        <label className="field-label">
          {t('workflows:workflowsPage.description')}
          <Textarea
            value={draft.description}
            onChange={(event) => onUpdateDraft({ description: event.target.value })}
          />
        </label>
        <label className="field-label">
          {t('workflows:workflowsPage.workingDirectory')}
          <Input
            value={draft.cwd}
            onChange={(event) => onUpdateDraft({ cwd: event.target.value })}
          />
        </label>
        <div className="form-grid three">
          <label className="field-label">
            {t('workflows:workflowsPage.visibility')}
            <span className="select-wrap">
              <AppSelect
                value={draft.visibility}
                onChange={(event) =>
                  onUpdateDraft({
                    visibility: event.target.value === 'shared' ? 'shared' : 'private',
                  })
                }
              >
                <option value="private">{t('workflows:workflowsPage.private')}</option>
                <option value="shared">{t('workflows:workflowsPage.shared')}</option>
              </AppSelect>
              <ChevronDown size={13} />
            </span>
          </label>
          <label className="field-label">
            {t('workflows:workflowsPage.tags')}
            <Input
              value={draft.tags.join(', ')}
              onChange={(event) =>
                onUpdateDraft({
                  tags: event.target.value
                    .split(',')
                    .map((value) => value.trim())
                    .filter(Boolean),
                })
              }
            />
          </label>
          <label className="field-label">
            {t('workflows:workflowsPage.revision')}
            <Input value={`v${draft.revision}`} disabled />
          </label>
        </div>
        <label className="field-label">
          {t('workflows:workflowsPage.defaultModel')}
          <span className="select-wrap">
            <AppSelect
              value={draft.model ? `${draft.model.provider}/${draft.model.model}` : ''}
              onChange={(event) => {
                const model = catalog.models.find(
                  (item) => `${item.provider}/${item.model}` === event.target.value,
                )
                onUpdateDraft({
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
        <div className="workflow-inputs-editor">
          <div className="card-head">
            <strong>{t('workflows:workflowsPage.inputParameters')}</strong>
            <Button
              size="icon-xs"
              variant="ghost"
              title={t('workflows:workflowsPage.addInput')}
              onClick={() =>
                onUpdateDraft({
                  inputs: [
                    ...draft.inputs,
                    {
                      id: crypto.randomUUID(),
                      name: `input_${draft.inputs.length + 1}`,
                      label: t('workflows:workflowsPage.newInput'),
                      type: 'string',
                      required: false,
                      defaultValue: '',
                      description: '',
                    },
                  ],
                })
              }
            >
              <Plus />
            </Button>
          </div>
          {draft.inputs.map((input) => (
            <div className="workflow-input-row" key={input.id}>
              <Input
                value={input.name}
                aria-label={t('workflows:workflowsPage.parameterName')}
                onChange={(event) =>
                  onUpdateDraft({
                    inputs: draft.inputs.map((item) =>
                      item.id === input.id ? { ...item, name: event.target.value } : item,
                    ),
                  })
                }
              />
              <AppSelect
                value={input.type}
                aria-label={t('workflows:workflowsPage.parameterType')}
                onChange={(event) =>
                  onUpdateDraft({
                    inputs: draft.inputs.map((item) =>
                      item.id === input.id
                        ? { ...item, type: event.target.value as WorkflowInputType }
                        : item,
                    ),
                  })
                }
              >
                <option value="string">String</option>
                <option value="text">Text</option>
                <option value="number">Number</option>
                <option value="boolean">Boolean</option>
              </AppSelect>
              <label className="workflow-required-input">
                <input
                  type="checkbox"
                  checked={input.required}
                  onChange={(event) =>
                    onUpdateDraft({
                      inputs: draft.inputs.map((item) =>
                        item.id === input.id ? { ...item, required: event.target.checked } : item,
                      ),
                    })
                  }
                />
                {t('workflows:workflowsPage.required')}
              </label>
              <Button
                size="icon-xs"
                variant="ghost"
                title={t('workflows:workflowsPage.delete')}
                onClick={() =>
                  onUpdateDraft({ inputs: draft.inputs.filter((item) => item.id !== input.id) })
                }
              >
                <Trash2 />
              </Button>
            </div>
          ))}
        </div>
        {!hasNotificationTarget && (
          <Alert className="workflow-notification-alert">
            <AlertTriangle />
            <AlertDescription>
              {t('workflows:workflowsPage.noNotificationChannelsEnabled')}
              <Button type="button" variant="link" size="sm" onClick={onOpenChannels}>
                {t('workflows:workflowsPage.openChannelSettings')}
              </Button>
            </AlertDescription>
          </Alert>
        )}
        {(
          Object.entries(NOTIFICATION_TARGETS) as Array<[NotificationTarget, { Icon: typeof Bell }]>
        ).map(([id, target]) => {
          const Icon = target.Icon
          const targetEnabled = Boolean(catalog.notificationTargets[id]?.enabled)
          return (
            <div className="toggle-line" key={id}>
              <span>
                <Icon size={15} />
                {notificationTargetLabel(id, t)}
              </span>
              <Switch
                checked={targetEnabled && draft.notifications.includes(id)}
                disabled={!targetEnabled}
                aria-label={notificationTargetLabel(id, t)}
                title={
                  targetEnabled
                    ? t('workflows:workflowsPage.notificationChannelEnabled')
                    : t('workflows:workflowsPage.notificationChannelNotEnabled')
                }
                onCheckedChange={() => onToggleNotification(id)}
              />
            </div>
          )
        })}
      </CardContent>
    </Card>
  )
}

function SelectedConnection({
  edge,
  nodes,
  t,
  onDelete,
}: {
  edge: WorkflowEdge
  nodes: WorkflowNode[]
  t: WorkflowTranslate
  onDelete: () => void
}) {
  const nodesById = new Map(nodes.map((node) => [node.id, node]))
  return (
    <Card size="sm" className="workflow-card gap-0 py-0">
      <CardContent className="p-3.5">
        <CardTitle className="workflow-section-title">
          {t('workflows:workflowsPage.selectedConnection')}
        </CardTitle>
        <div className="workflow-edge-summary">
          <strong>
            {nodesById.get(edge.source)?.label || t('workflows:workflowsPage.unknownNode')}
          </strong>
          <span>→</span>
          <strong>
            {nodesById.get(edge.target)?.label || t('workflows:workflowsPage.unknownNode')}
          </strong>
        </div>
        <p className="muted-copy">
          {t('workflows:workflowsPage.pressDeleteOrBackspaceToRemoveThisConnection')}
        </p>
        <Button size="sm" variant="destructive" onClick={onDelete}>
          <Trash2 data-icon="inline-start" />
          {t('workflows:workflowsPage.deleteConnection')}
        </Button>
      </CardContent>
    </Card>
  )
}

function SelectedNode({
  node,
  selectedEdge,
  catalog,
  t,
  onUpdateNode,
  onCopy,
  onDelete,
}: {
  node: WorkflowNode | null
  selectedEdge: WorkflowEdge | null
  catalog: WorkflowsData
  t: WorkflowTranslate
  onUpdateNode: (patch: Partial<WorkflowNode>) => void
  onCopy: () => void
  onDelete: () => void
}) {
  return (
    <Card size="sm" className="workflow-card gap-0 py-0">
      <CardContent className="p-3.5">
        <CardTitle className="workflow-section-title">
          {t('workflows:workflowsPage.selectedNode')}
        </CardTitle>
        {node ? (
          <>
            <label className="field-label">
              {t('workflows:workflowsPage.nodeName')}
              <Input
                value={node.label}
                onChange={(event) => onUpdateNode({ label: event.target.value })}
              />
            </label>
            <label className="field-label">
              {t('workflows:workflowsPage.nodeModel')}
              <span className="select-wrap">
                <AppSelect
                  value={node.model ? `${node.model.provider}/${node.model.model}` : ''}
                  onChange={(event) => {
                    const model = catalog.models.find(
                      (item) => `${item.provider}/${item.model}` === event.target.value,
                    )
                    onUpdateNode({
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
                <Input
                  type="number"
                  min="0"
                  max="3"
                  value={node.retries}
                  onChange={(event) => onUpdateNode({ retries: Number(event.target.value) })}
                />
              </label>
              <label className="field-label">
                {t('workflows:workflowsPage.timeoutMinutes')}
                <Input
                  type="number"
                  min="1"
                  max="240"
                  value={node.timeoutMinutes}
                  onChange={(event) => onUpdateNode({ timeoutMinutes: Number(event.target.value) })}
                />
              </label>
              <label className="field-label">
                {t('workflows:workflowsPage.failureHandling')}
                <span className="select-wrap">
                  <AppSelect
                    value={node.failurePolicy}
                    onChange={(event) =>
                      onUpdateNode({
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
            {['prompt', 'skill', 'file', 'mcp'].includes(node.kind) && (
              <>
                {node.kind === 'skill' && (
                  <label className="field-label">
                    Skill
                    <span className="select-wrap">
                      <AppSelect
                        value={node.skillName}
                        onChange={(event) => onUpdateNode({ skillName: event.target.value })}
                      >
                        <option value="">{t('workflows:workflowsPage.chooseSkill')}</option>
                        {catalog.skills.map((skill) => (
                          <option value={skill.name} key={skill.id}>
                            {skill.name}
                          </option>
                        ))}
                      </AppSelect>
                      <ChevronDown size={13} />
                    </span>
                  </label>
                )}
                {node.kind === 'mcp' && (
                  <label className="field-label">
                    {t('workflows:workflowsPage.mcpToolNames')}
                    <Input
                      value={node.requestedToolNames.join(', ')}
                      onChange={(event) =>
                        onUpdateNode({
                          requestedToolNames: event.target.value
                            .split(',')
                            .map((value) => value.trim())
                            .filter(Boolean),
                        })
                      }
                      placeholder="server.tool_name"
                    />
                  </label>
                )}
                <label className="field-label">
                  Prompt
                  <Textarea
                    value={node.prompt}
                    onChange={(event) => onUpdateNode({ prompt: event.target.value })}
                    placeholder={t(
                      'workflows:workflowsPage.describeTheWorkTheAgentShouldCompleteInThisNode',
                    )}
                  />
                </label>
                <label className="field-label">
                  {t('workflows:workflowsPage.outputFormat')}
                  <span className="select-wrap">
                    <AppSelect
                      value={node.outputFormat}
                      onChange={(event) =>
                        onUpdateNode({
                          outputFormat: event.target.value === 'json' ? 'json' : 'text',
                        })
                      }
                    >
                      <option value="text">Text</option>
                      <option value="json">JSON</option>
                    </AppSelect>
                    <ChevronDown size={13} />
                  </span>
                </label>
              </>
            )}
            {node.kind === 'condition' && (
              <div className="form-grid three">
                <label className="field-label">
                  {t('workflows:workflowsPage.dataPath')}
                  <Input
                    value={node.condition.source}
                    onChange={(event) =>
                      onUpdateNode({ condition: { ...node.condition, source: event.target.value } })
                    }
                    placeholder="inputs.approved"
                  />
                </label>
                <label className="field-label">
                  {t('workflows:workflowsPage.operator')}
                  <span className="select-wrap">
                    <AppSelect
                      value={node.condition.operator}
                      onChange={(event) =>
                        onUpdateNode({
                          condition: {
                            ...node.condition,
                            operator: event.target.value as typeof node.condition.operator,
                          },
                        })
                      }
                    >
                      {[
                        'exists',
                        'not_exists',
                        'equals',
                        'not_equals',
                        'contains',
                        'greater_than',
                        'less_than',
                      ].map((operator) => (
                        <option value={operator} key={operator}>
                          {operator}
                        </option>
                      ))}
                    </AppSelect>
                    <ChevronDown size={13} />
                  </span>
                </label>
                <label className="field-label">
                  {t('workflows:workflowsPage.comparisonValue')}
                  <Input
                    value={String(node.condition.value ?? '')}
                    onChange={(event) =>
                      onUpdateNode({ condition: { ...node.condition, value: event.target.value } })
                    }
                  />
                </label>
              </div>
            )}
            {node.kind === 'approval' && (
              <>
                <label className="field-label">
                  {t('workflows:workflowsPage.approvalMessage')}
                  <Textarea
                    value={node.approval.message}
                    onChange={(event) =>
                      onUpdateNode({ approval: { ...node.approval, message: event.target.value } })
                    }
                  />
                </label>
                <label className="field-label">
                  {t('workflows:workflowsPage.approvalTimeout')}
                  <Input
                    type="number"
                    min="1"
                    max="10080"
                    value={node.approval.timeoutMinutes}
                    onChange={(event) =>
                      onUpdateNode({
                        approval: { ...node.approval, timeoutMinutes: Number(event.target.value) },
                      })
                    }
                  />
                </label>
              </>
            )}
            <div className="button-row">
              <Button size="sm" variant="secondary" onClick={onCopy}>
                <Copy data-icon="inline-start" />
                {t('workflows:workflowsPage.duplicateNode')}
              </Button>
              <Button size="sm" variant="destructive" onClick={onDelete}>
                <Trash2 data-icon="inline-start" />
                {t('workflows:workflowsPage.deleteNode')}
              </Button>
            </div>
          </>
        ) : (
          <p className="muted-copy">
            {selectedEdge
              ? t('workflows:workflowsPage.aConnectionIsCurrentlySelected')
              : t('workflows:workflowsPage.dragNodesFromTheLeftToStartBuildingTheWorkflow')}
          </p>
        )}
      </CardContent>
    </Card>
  )
}

export function WorkflowNodeInspector({
  draft,
  catalog,
  selectedNode,
  selectedEdge,
  currentRun,
  language,
  t,
  onUpdateDraft,
  onUpdateNode,
  onToggleNotification,
  onDeleteEdge,
  onCopyNode,
  onDeleteNode,
  onOpenChannels,
}: {
  draft: Workflow
  catalog: WorkflowsData
  selectedNode: WorkflowNode | null
  selectedEdge: WorkflowEdge | null
  currentRun?: WorkflowRun
  language: string
  t: WorkflowTranslate
  onUpdateDraft: (patch: Partial<Workflow>) => void
  onUpdateNode: (patch: Partial<WorkflowNode>) => void
  onToggleNotification: (target: NotificationTarget) => void
  onDeleteEdge: () => void
  onCopyNode: () => void
  onDeleteNode: () => void
  onOpenChannels: () => void
}) {
  return (
    <div className="detail-stack inspector">
      <WorkflowSettings
        draft={draft}
        catalog={catalog}
        t={t}
        onUpdateDraft={onUpdateDraft}
        onToggleNotification={onToggleNotification}
        onOpenChannels={onOpenChannels}
      />
      {selectedEdge && (
        <SelectedConnection edge={selectedEdge} nodes={draft.nodes} t={t} onDelete={onDeleteEdge} />
      )}
      <SelectedNode
        node={selectedNode}
        selectedEdge={selectedEdge}
        catalog={catalog}
        t={t}
        onUpdateNode={onUpdateNode}
        onCopy={onCopyNode}
        onDelete={onDeleteNode}
      />
      {draft.id && <WorkflowLatestRun run={currentRun} language={language} t={t} />}
    </div>
  )
}
