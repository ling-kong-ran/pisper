// 工作流节点检查器：选中节点后的属性编辑（提示词/技能/触发器等），
// 校验必填字段并就地写回工作流。
import { AlertTriangle, Bell, Bot, Copy, MessageCircle, Plus, Trash2 } from 'lucide-react'
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
  WorkflowExecutionMode,
  WorkflowNode,
  WorkflowInputType,
  WorkflowRun,
  WorkflowsData,
} from './types'
import { WorkflowLatestRun } from './WorkflowRunControls'
import type { DesktopNotificationPermission } from '@/types/update'
import type { WorkflowTranslate } from './workflow-templates'

import { FieldLabel } from '@/components/ui/field'

import { AppCardHeader } from '@/components/ui/app-primitives'

const NOTIFICATION_TARGETS = {
  browser: { Icon: Bell },
  feishu: { Icon: Bot },
  weixin: { Icon: MessageCircle },
}

const WORKFLOW_EXECUTION_MODES: WorkflowExecutionMode[] = [
  'read-only',
  'workspace-write',
  'full-access',
]

function executionModeLabel(mode: WorkflowExecutionMode, t: WorkflowTranslate) {
  if (mode === 'read-only') return t('workflows:workflowsPage.readOnly')
  if (mode === 'workspace-write') return t('workflows:workflowsPage.workspaceWrite')
  return t('workflows:workflowsPage.fullAccess')
}

function executionModeHelp(mode: WorkflowExecutionMode, t: WorkflowTranslate) {
  if (mode === 'read-only') return t('workflows:workflowsPage.readOnlyHelp')
  if (mode === 'workspace-write') return t('workflows:workflowsPage.workspaceWriteHelp')
  return t('workflows:workflowsPage.fullAccessHelp')
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
}: {
  draft: Workflow
  catalog: WorkflowsData
  t: WorkflowTranslate
  onUpdateDraft: (patch: Partial<Workflow>) => void
}) {
  return (
    <Card
      size="sm"
      className="workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] gap-0 py-0"
    >
      <CardContent className="p-3.5">
        <CardTitle className="workflow-section-title [.selection-list_&]:mb-[8px] [.node-library_&]:mb-[8px] text-[var(--text-soft)] text-[13px] font-[700] leading-[1.4]">
          {t('workflows:workflowsPage.workflowSettings')}
        </CardTitle>
        <FieldLabel variant="control">
          {t('workflows:workflowsPage.name')}
          <Input
            value={draft.name}
            onChange={(event) => onUpdateDraft({ name: event.target.value })}
          />
        </FieldLabel>
        <FieldLabel variant="control">
          {t('workflows:workflowsPage.description')}
          <Textarea
            value={draft.description}
            onChange={(event) => onUpdateDraft({ description: event.target.value })}
          />
        </FieldLabel>
        <FieldLabel variant="control">
          {t('workflows:workflowsPage.workingDirectory')}
          <Input
            value={draft.cwd}
            onChange={(event) => onUpdateDraft({ cwd: event.target.value })}
          />
        </FieldLabel>
        <div className="form-grid grid gap-[9px] three [.form-grid&]:grid-cols-[repeat(3,minmax(0,1fr))] max-[650px]:[.form-grid&]:grid-cols-[1fr]">
          <FieldLabel variant="control">
            {t('workflows:workflowsPage.visibility')}
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
          </FieldLabel>
          <FieldLabel variant="control">
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
          </FieldLabel>
          <FieldLabel variant="control">
            {t('workflows:workflowsPage.revision')}
            <Input value={`v${draft.revision}`} disabled />
          </FieldLabel>
        </div>
        <FieldLabel variant="control">
          {t('workflows:workflowsPage.defaultModel')}
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
        </FieldLabel>
        <div className="workflow-inputs-editor">
          <AppCardHeader>
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
          </AppCardHeader>
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
      </CardContent>
    </Card>
  )
}

function NodeNotificationSettings({
  node,
  catalog,
  t,
  systemNotificationPermission,
  onUpdateNode,
  onToggleNotification,
  onOpenChannels,
  onOpenSystemNotificationSettings,
}: {
  node: WorkflowNode
  catalog: WorkflowsData
  t: WorkflowTranslate
  systemNotificationPermission: DesktopNotificationPermission
  onUpdateNode: (patch: Partial<WorkflowNode>) => void
  onToggleNotification: (target: NotificationTarget) => void | Promise<void>
  onOpenChannels: () => void
  onOpenSystemNotificationSettings: () => void
}) {
  const hasExternalNotificationTarget = ['feishu', 'weixin'].some(
    (target) => catalog.notificationTargets[target as NotificationTarget]?.enabled,
  )
  const systemNotificationAvailable =
    systemNotificationPermission === 'default' || systemNotificationPermission === 'granted'

  return (
    <>
      <FieldLabel variant="control">
        {t('workflows:workflowsPage.notificationTitle')}
        <Input
          value={node.notification.title}
          onChange={(event) =>
            onUpdateNode({ notification: { ...node.notification, title: event.target.value } })
          }
          placeholder="{{workflow.name}}"
        />
      </FieldLabel>
      <FieldLabel variant="control">
        {t('workflows:workflowsPage.notificationContent')}
        <Textarea
          value={node.notification.content}
          onChange={(event) =>
            onUpdateNode({ notification: { ...node.notification, content: event.target.value } })
          }
          placeholder="{{inputs.name}} · {{previous.summary}}"
        />
      </FieldLabel>
      <strong className="block [margin-top:12px] text-[var(--text-secondary)] text-[12px]">
        {t('workflows:workflowsPage.notificationChannels')}
      </strong>
      {!hasExternalNotificationTarget && (
        <Alert className="workflow-notification-alert [&_[data-slot='alert-description']]:flex [&_[data-slot='alert-description']]:items-center [&_[data-slot='alert-description']]:gap-[4px] [&_[data-slot='alert-description']]:text-[11px] [&_[data-slot='alert-description']]:leading-[1.4] [margin:10px_0_6px] [border-color:color-mix(in_srgb,var(--warning)_35%,var(--border))] bg-[color-mix(in_srgb,var(--warning)_7%,var(--card))] text-[var(--text-secondary)]">
          <AlertTriangle />
          <AlertDescription>
            {t('workflows:workflowsPage.noExternalNotificationChannelsEnabled')}
            <Button type="button" variant="link" size="sm" onClick={onOpenChannels}>
              {t('workflows:workflowsPage.openChannelSettings')}
            </Button>
          </AlertDescription>
        </Alert>
      )}
      {(systemNotificationPermission === 'denied' ||
        systemNotificationPermission === 'unsupported') && (
        <Alert className="workflow-notification-alert [&_[data-slot='alert-description']]:flex [&_[data-slot='alert-description']]:items-center [&_[data-slot='alert-description']]:gap-[4px] [&_[data-slot='alert-description']]:text-[11px] [&_[data-slot='alert-description']]:leading-[1.4] [margin:10px_0_6px] [border-color:color-mix(in_srgb,var(--warning)_35%,var(--border))] bg-[color-mix(in_srgb,var(--warning)_7%,var(--card))] text-[var(--text-secondary)]">
          <AlertTriangle />
          <AlertDescription>
            {systemNotificationPermission === 'unsupported'
              ? t('workflows:workflowsPage.systemNotificationsUnsupported')
              : t('workflows:workflowsPage.systemNotificationPermissionRequired')}
            <Button
              type="button"
              variant="link"
              size="sm"
              onClick={onOpenSystemNotificationSettings}
            >
              {t('workflows:workflowsPage.openSystemNotificationSettings')}
            </Button>
          </AlertDescription>
        </Alert>
      )}
      {(
        Object.entries(NOTIFICATION_TARGETS) as Array<[NotificationTarget, { Icon: typeof Bell }]>
      ).map(([id, target]) => {
        const Icon = target.Icon
        const targetEnabled =
          id === 'browser'
            ? systemNotificationAvailable
            : Boolean(catalog.notificationTargets[id]?.enabled)
        return (
          <div
            className="toggle-line [&_>_span]:flex [&_>_span]:items-center [&_>_span]:gap-[7px] [&_>_span]:text-[12px] flex min-h-[34px] items-center justify-between [border-top:1px_solid_var(--stroke-soft)]"
            key={id}
          >
            <span>
              <Icon size={15} />
              {notificationTargetLabel(id, t)}
            </span>
            <Switch
              checked={
                targetEnabled &&
                (id !== 'browser' ||
                  (systemNotificationPermission === 'granted' &&
                    catalog.notificationTargets.browser.enabled)) &&
                node.notificationTargets.includes(id)
              }
              disabled={!targetEnabled}
              aria-label={notificationTargetLabel(id, t)}
              title={
                id === 'browser'
                  ? targetEnabled
                    ? catalog.notificationTargets.browser.enabled
                      ? t('workflows:workflowsPage.systemNotificationsEnabled')
                      : t('workflows:workflowsPage.systemNotificationReady')
                    : t('workflows:workflowsPage.systemNotificationPermissionRequired')
                  : targetEnabled
                    ? t('workflows:workflowsPage.notificationChannelEnabled')
                    : t('workflows:workflowsPage.notificationChannelNotEnabled')
              }
              onCheckedChange={() => void onToggleNotification(id)}
            />
          </div>
        )
      })}
    </>
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
    <Card
      size="sm"
      className="workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] gap-0 py-0"
    >
      <CardContent className="p-3.5">
        <CardTitle className="workflow-section-title [.selection-list_&]:mb-[8px] [.node-library_&]:mb-[8px] text-[var(--text-soft)] text-[13px] font-[700] leading-[1.4]">
          {t('workflows:workflowsPage.selectedConnection')}
        </CardTitle>
        <div className="workflow-edge-summary [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_span]:text-[var(--text-muted)] grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-[8px] [margin:10px_0]">
          <strong>
            {nodesById.get(edge.source)?.label || t('workflows:workflowsPage.unknownNode')}
          </strong>
          <span>→</span>
          <strong>
            {nodesById.get(edge.target)?.label || t('workflows:workflowsPage.unknownNode')}
          </strong>
        </div>
        <p className="muted-copy m-[8px_0_14px] text-[var(--text-muted)] text-[12px] leading-[1.55]">
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
  systemNotificationPermission,
  onToggleNotification,
  onCopy,
  onDelete,
  onOpenChannels,
  onOpenSystemNotificationSettings,
}: {
  node: WorkflowNode | null
  selectedEdge: WorkflowEdge | null
  catalog: WorkflowsData
  t: WorkflowTranslate
  onUpdateNode: (patch: Partial<WorkflowNode>) => void
  systemNotificationPermission: DesktopNotificationPermission
  onToggleNotification: (target: NotificationTarget) => void | Promise<void>
  onCopy: () => void
  onDelete: () => void
  onOpenChannels: () => void
  onOpenSystemNotificationSettings: () => void
}) {
  return (
    <Card
      size="sm"
      className="workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] gap-0 py-0"
    >
      <CardContent className="p-3.5">
        <CardTitle className="workflow-section-title [.selection-list_&]:mb-[8px] [.node-library_&]:mb-[8px] text-[var(--text-soft)] text-[13px] font-[700] leading-[1.4]">
          {t('workflows:workflowsPage.selectedNode')}
        </CardTitle>
        {node ? (
          <>
            <FieldLabel variant="control">
              {t('workflows:workflowsPage.nodeName')}
              <Input
                value={node.label}
                onChange={(event) => onUpdateNode({ label: event.target.value })}
              />
            </FieldLabel>
            <FieldLabel variant="control">
              {t('workflows:workflowsPage.nodeModel')}
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
                <option value="">{t('workflows:workflowsPage.inheritWorkflowDefaultModel')}</option>
                {catalog.models.map((model) => (
                  <option
                    value={`${model.provider}/${model.model}`}
                    key={`${model.provider}/${model.model}`}
                  >
                    {model.label}
                  </option>
                ))}
              </AppSelect>
            </FieldLabel>
            <div className="form-grid grid gap-[9px] three [.form-grid&]:grid-cols-[repeat(3,minmax(0,1fr))] max-[650px]:[.form-grid&]:grid-cols-[1fr]">
              <FieldLabel variant="control">
                {t('workflows:workflowsPage.retryCount')}
                <Input
                  type="number"
                  min="0"
                  max="3"
                  value={node.retries}
                  onChange={(event) => onUpdateNode({ retries: Number(event.target.value) })}
                />
              </FieldLabel>
              <FieldLabel variant="control">
                {t('workflows:workflowsPage.timeoutMinutes')}
                <Input
                  type="number"
                  min="1"
                  max="240"
                  value={node.timeoutMinutes}
                  onChange={(event) => onUpdateNode({ timeoutMinutes: Number(event.target.value) })}
                />
              </FieldLabel>
              <FieldLabel variant="control">
                {t('workflows:workflowsPage.failureHandling')}
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
              </FieldLabel>
            </div>
            {['prompt', 'skill', 'file', 'mcp'].includes(node.kind) && (
              <>
                <FieldLabel variant="control">
                  {t('workflows:workflowsPage.executionMode')}
                  <AppSelect
                    value={node.executionMode}
                    onChange={(event) =>
                      onUpdateNode({
                        executionMode: event.target.value as WorkflowExecutionMode,
                      })
                    }
                  >
                    {WORKFLOW_EXECUTION_MODES.map((mode) => (
                      <option value={mode} key={mode}>
                        {executionModeLabel(mode, t)}
                      </option>
                    ))}
                  </AppSelect>
                  <small>{executionModeHelp(node.executionMode, t)}</small>
                </FieldLabel>
                {node.kind === 'skill' && (
                  <FieldLabel variant="control">
                    Skill
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
                  </FieldLabel>
                )}
                {node.kind === 'mcp' && (
                  <FieldLabel variant="control">
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
                  </FieldLabel>
                )}
                <FieldLabel variant="control">
                  Prompt
                  <Textarea
                    value={node.prompt}
                    onChange={(event) => onUpdateNode({ prompt: event.target.value })}
                    placeholder={t(
                      'workflows:workflowsPage.describeTheWorkTheAgentShouldCompleteInThisNode',
                    )}
                  />
                </FieldLabel>
                <FieldLabel variant="control">
                  {t('workflows:workflowsPage.outputFormat')}
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
                </FieldLabel>
              </>
            )}
            {node.kind === 'condition' && (
              <div className="form-grid grid gap-[9px] three [.form-grid&]:grid-cols-[repeat(3,minmax(0,1fr))] max-[650px]:[.form-grid&]:grid-cols-[1fr]">
                <FieldLabel variant="control">
                  {t('workflows:workflowsPage.dataPath')}
                  <Input
                    value={node.condition.source}
                    onChange={(event) =>
                      onUpdateNode({ condition: { ...node.condition, source: event.target.value } })
                    }
                    placeholder="inputs.approved"
                  />
                </FieldLabel>
                <FieldLabel variant="control">
                  {t('workflows:workflowsPage.operator')}
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
                </FieldLabel>
                <FieldLabel variant="control">
                  {t('workflows:workflowsPage.comparisonValue')}
                  <Input
                    value={String(node.condition.value ?? '')}
                    onChange={(event) =>
                      onUpdateNode({ condition: { ...node.condition, value: event.target.value } })
                    }
                  />
                </FieldLabel>
              </div>
            )}
            {node.kind === 'notification' && (
              <NodeNotificationSettings
                node={node}
                catalog={catalog}
                t={t}
                systemNotificationPermission={systemNotificationPermission}
                onUpdateNode={onUpdateNode}
                onToggleNotification={onToggleNotification}
                onOpenChannels={onOpenChannels}
                onOpenSystemNotificationSettings={onOpenSystemNotificationSettings}
              />
            )}
            {node.kind === 'approval' && (
              <>
                <FieldLabel variant="control">
                  {t('workflows:workflowsPage.approvalMessage')}
                  <Textarea
                    value={node.approval.message}
                    onChange={(event) =>
                      onUpdateNode({ approval: { ...node.approval, message: event.target.value } })
                    }
                  />
                </FieldLabel>
                <FieldLabel variant="control">
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
                </FieldLabel>
              </>
            )}
            <div className="mt-[15px] flex gap-2 max-[650px]:flex-wrap">
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
          <p className="muted-copy m-[8px_0_14px] text-[var(--text-muted)] text-[12px] leading-[1.55]">
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
  systemNotificationPermission,
  onToggleNotification,
  onDeleteEdge,
  onCopyNode,
  onDeleteNode,
  onOpenChannels,
  onOpenSystemNotificationSettings,
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
  systemNotificationPermission: DesktopNotificationPermission
  onToggleNotification: (target: NotificationTarget) => void | Promise<void>
  onDeleteEdge: () => void
  onCopyNode: () => void
  onDeleteNode: () => void
  onOpenChannels: () => void
  onOpenSystemNotificationSettings: () => void
}) {
  return (
    <div className="detail-stack flex min-w-0 flex-col gap-[12px] [.mcp-layout_>_&]:min-h-0 max-[1150px]:[.memory-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.memory-layout_>_&]:grid max-[1150px]:[.memory-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[1150px]:[.mcp-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.mcp-layout_>_&]:grid max-[1150px]:[.mcp-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[1150px]:[.skills-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.skills-layout_>_&]:grid max-[1150px]:[.skills-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[650px]:[.memory-layout_>_&]:[grid-column:auto] max-[650px]:[.memory-layout_>_&]:grid-cols-[1fr] max-[650px]:[.mcp-layout_>_&]:[grid-column:auto] max-[650px]:[.mcp-layout_>_&]:grid-cols-[1fr] max-[650px]:[.skills-layout_>_&]:[grid-column:auto] max-[650px]:[.skills-layout_>_&]:grid-cols-[1fr] inspector !min-w-0 max-[1150px]:[.builder-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.builder-layout_>_&]:grid max-[1150px]:[.builder-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[900px]:[.builder-layout_>_&]:[grid-column:1/-1]">
      <WorkflowSettings draft={draft} catalog={catalog} t={t} onUpdateDraft={onUpdateDraft} />
      {selectedEdge && (
        <SelectedConnection edge={selectedEdge} nodes={draft.nodes} t={t} onDelete={onDeleteEdge} />
      )}
      <SelectedNode
        node={selectedNode}
        selectedEdge={selectedEdge}
        catalog={catalog}
        t={t}
        onUpdateNode={onUpdateNode}
        systemNotificationPermission={systemNotificationPermission}
        onToggleNotification={onToggleNotification}
        onCopy={onCopyNode}
        onDelete={onDeleteNode}
        onOpenChannels={onOpenChannels}
        onOpenSystemNotificationSettings={onOpenSystemNotificationSettings}
      />
      {draft.id && <WorkflowLatestRun run={currentRun} language={language} t={t} />}
    </div>
  )
}
