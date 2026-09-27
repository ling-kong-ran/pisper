import { isWorkflowImageNodeKind } from '@shared/workflow-image-nodes.mjs'
import { WorkflowImageNodeInspector } from './WorkflowImageNodeInspector'
import { WorkflowImageResult } from './WorkflowImageResult'
// 工作流节点检查器：选中节点后的属性编辑（提示词/技能/触发器等），
// 校验必填字段并就地写回工作流。
import { AlertTriangle, Bell, Bot, Copy, MessageCircle, Play, Send, Trash2 } from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import type {
  NotificationTarget,
  Workflow,
  WorkflowEdge,
  WorkflowExecutionMode,
  WorkflowNode,
  WorkflowInput,
  WorkflowRun,
  WorkflowsData,
} from './types'
import { WorkflowLatestRun } from './WorkflowRunControls'
import type { DesktopNotificationPermission } from '@/types/update'
import type { WorkflowTranslate } from './workflow-templates'

import { FieldLabel } from '@/components/ui/field'

import { WorkflowInputsEditor } from './WorkflowInputsEditor'
import { useRef } from 'react'

const NOTIFICATION_TARGETS = {
  browser: { Icon: Bell },
  feishu: { Icon: Bot },
  weixin: { Icon: MessageCircle },
  qq: { Icon: MessageCircle },
  telegram: { Icon: Send },
}

const WORKFLOW_EXECUTION_MODES: WorkflowExecutionMode[] = ['workspace-write', 'full-access']

function executionModeLabel(mode: WorkflowExecutionMode, t: WorkflowTranslate) {
  if (mode === 'workspace-write') return t('workflows:workflowsPage.workspaceWrite')
  return t('workflows:workflowsPage.fullAccess')
}

function executionModeHelp(mode: WorkflowExecutionMode, t: WorkflowTranslate) {
  if (mode === 'workspace-write') return t('workflows:workflowsPage.workspaceWriteHelp')
  return t('workflows:workflowsPage.fullAccessHelp')
}

function notificationTargetLabel(target: NotificationTarget, t: WorkflowTranslate) {
  if (target === 'feishu') return t('workflows:workflowsPage.feishu')
  if (target === 'weixin') return t('workflows:workflowsPage.weChat')
  if (target === 'qq') return t('workflows:workflowsPage.qq')
  if (target === 'telegram') return t('workflows:workflowsPage.telegram')
  return t('workflows:workflowsPage.browserNotification')
}

export function WorkflowSettings({
  draft,
  catalog,
  t,
  onUpdateDraft,
  onInputUploadBusy,
}: {
  draft: Workflow
  catalog: WorkflowsData
  t: WorkflowTranslate
  onUpdateDraft: (patch: Partial<Workflow>) => void
  onInputUploadBusy?: (id: string, busy: boolean) => void
}) {
  return (
    <div className="min-w-0 space-y-4">
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
        <Input value={draft.cwd} onChange={(event) => onUpdateDraft({ cwd: event.target.value })} />
      </FieldLabel>
      <div className="grid gap-3 sm:grid-cols-3">
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
      <WorkflowInputsEditor
        inputs={draft.inputs}
        t={t}
        onUploadBusy={onInputUploadBusy}
        onChange={(inputs) => onUpdateDraft({ inputs })}
      />
    </div>
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
  const hasExternalNotificationTarget = ['feishu', 'weixin', 'qq', 'telegram'].some(
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
    <section className="min-w-0 space-y-3">
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
    </section>
  )
}

function SelectedNode({
  node,
  inputs,
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
  inputs: WorkflowInput[]
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
  const promptInput = useRef<HTMLTextAreaElement>(null)
  return (
    <section className="min-w-0 space-y-3">
      {node ? (
        <>
          <FieldLabel variant="control">
            {t('workflows:workflowsPage.nodeName')}
            <Input
              value={node.label}
              onChange={(event) => onUpdateNode({ label: event.target.value })}
            />
          </FieldLabel>
          {['prompt', 'skill', 'file', 'mcp'].includes(node.kind) && (
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
          )}
          {node.kind === 'trigger' && (
            <p className="rounded-lg bg-muted/50 p-3 text-xs leading-relaxed text-muted-foreground">
              {t('workflows:editor.manualTriggerHint')}
            </p>
          )}
          {node.kind === 'parallel' && (
            <p className="rounded-lg bg-muted/50 p-3 text-xs leading-relaxed text-muted-foreground">
              {t('workflows:editor.parallelHint')}
            </p>
          )}
          {!['trigger', 'parallel', 'condition', 'approval'].includes(node.kind) && (
            <details className="rounded-lg border p-3">
              <summary className="cursor-pointer text-xs font-medium">
                {t('workflows:editor.executionSettings')}
              </summary>
              <div className="grid gap-3 pt-3">
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
                    onChange={(event) =>
                      onUpdateNode({ timeoutMinutes: Number(event.target.value) })
                    }
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
            </details>
          )}
          {isWorkflowImageNodeKind(node.kind) && (
            <WorkflowImageNodeInspector node={node} inputs={inputs} t={t} onChange={onUpdateNode} />
          )}
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
                {t('workflows:inputs.nodeRules')}
                <Textarea
                  ref={promptInput}
                  value={node.prompt}
                  onChange={(event) => onUpdateNode({ prompt: event.target.value })}
                  placeholder={t(
                    'workflows:workflowsPage.describeTheWorkTheAgentShouldCompleteInThisNode',
                  )}
                />
              </FieldLabel>
              <div className="space-y-2 py-2">
                <p className="text-xs leading-relaxed text-muted-foreground">
                  {t('workflows:inputs.nodeRulesHint')}
                </p>
                <div className="flex flex-wrap gap-1.5">
                  {inputs.map((input) => (
                    <Button
                      type="button"
                      key={input.id}
                      variant="outline"
                      size="xs"
                      title={t('workflows:inputs.insertVariable', { name: input.label })}
                      onClick={() => {
                        const textarea = promptInput.current
                        const start = textarea?.selectionStart ?? node.prompt.length
                        const end = textarea?.selectionEnd ?? start
                        const variable = '{{inputs.' + input.name + '}}'
                        onUpdateNode({
                          prompt: node.prompt.slice(0, start) + variable + node.prompt.slice(end),
                        })
                        requestAnimationFrame(() => {
                          textarea?.focus()
                          textarea?.setSelectionRange(
                            start + variable.length,
                            start + variable.length,
                          )
                        })
                      }}
                    >
                      <code>{'{{inputs.' + input.name + '}}'}</code>
                    </Button>
                  ))}
                </div>
              </div>
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
            <div className="grid gap-3">
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
    </section>
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
  onUpdateNode,
  systemNotificationPermission,
  onToggleNotification,
  onDeleteEdge,
  onCopyNode,
  onDeleteNode,
  onOpenChannels,
  onOpenSystemNotificationSettings,
  onRunImageNode,
  imageRunBusy = false,
}: {
  draft: Workflow
  catalog: WorkflowsData
  selectedNode: WorkflowNode | null
  selectedEdge: WorkflowEdge | null
  currentRun?: WorkflowRun
  onRunImageNode?: (nodeId: string, sourceRunId: string) => void
  imageRunBusy?: boolean
  language: string
  t: WorkflowTranslate
  onUpdateNode: (patch: Partial<WorkflowNode>) => void
  systemNotificationPermission: DesktopNotificationPermission
  onToggleNotification: (target: NotificationTarget) => void | Promise<void>
  onDeleteEdge: () => void
  onCopyNode: () => void
  onDeleteNode: () => void
  onOpenChannels: () => void
  onOpenSystemNotificationSettings: () => void
}) {
  const upstreamIds = draft.edges
    .filter((edge) => edge.target === selectedNode?.id)
    .map((edge) => edge.source)
  const sourceRun = (currentRun ? [currentRun] : []).find(
    (run) =>
      run.workflowId === draft.id &&
      (selectedNode?.kind === 'media-input'
        ? Boolean(run.inputs)
        : upstreamIds.length > 0 &&
          upstreamIds.every((id) =>
            run.nodes?.some((node) => node.id === id && node.status === 'completed' && node.output),
          )),
  )
  const resultRun = currentRun
  return (
    <div className="detail-stack min-w-0 space-y-5">
      {selectedEdge && (
        <SelectedConnection edge={selectedEdge} nodes={draft.nodes} t={t} onDelete={onDeleteEdge} />
      )}
      {!selectedEdge && (
        <SelectedNode
          node={selectedNode}
          inputs={draft.inputs}
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
      )}
      {selectedNode && isWorkflowImageNodeKind(selectedNode.kind) && onRunImageNode && (
        <div className="space-y-2 rounded-lg border p-3">
          <Button
            className="w-full"
            variant="outline"
            disabled={!draft.id || !sourceRun || imageRunBusy}
            onClick={() => {
              if (sourceRun) onRunImageNode(selectedNode.id, sourceRun.id)
            }}
          >
            <Play />
            {t('workflows:imageNodes.runNode')}
          </Button>
          <p className="text-xs text-muted-foreground">{t('workflows:imageNodes.runNodeHint')}</p>
        </div>
      )}
      {selectedNode && isWorkflowImageNodeKind(selectedNode.kind) && (
        <WorkflowImageResult
          key={selectedNode.id}
          value={resultRun?.nodes?.find((node) => node.id === selectedNode.id)?.output}
          previousValue={
            sourceRun?.nodes?.find(
              (node) =>
                node.id === draft.edges.find((edge) => edge.target === selectedNode.id)?.source,
            )?.output
          }
          error={resultRun?.nodes?.find((node) => node.id === selectedNode.id)?.error}
          t={t}
        />
      )}
      {draft.id && currentRun && (
        <details className="rounded-lg border p-3">
          <summary className="cursor-pointer text-sm font-medium">
            {t('workflows:workflowsPage.latestRun')}
          </summary>
          <div className="mt-3">
            <WorkflowLatestRun run={currentRun} language={language} t={t} />
          </div>
        </details>
      )}
    </div>
  )
}
