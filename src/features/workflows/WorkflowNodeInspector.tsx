import { Bell, Bot, ChevronDown, Copy, MessageCircle, Trash2 } from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
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
}: {
  draft: Workflow
  catalog: WorkflowsData
  t: WorkflowTranslate
  onUpdateDraft: (patch: Partial<Workflow>) => void
  onToggleNotification: (target: NotificationTarget) => void
}) {
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
        {(
          Object.entries(NOTIFICATION_TARGETS) as Array<[NotificationTarget, { Icon: typeof Bell }]>
        ).map(([id, target]) => {
          const Icon = target.Icon
          return (
            <div className="toggle-line" key={id}>
              <span>
                <Icon size={15} />
                {notificationTargetLabel(id, t)}
              </span>
              <Switch
                size="sm"
                checked={draft.notifications.includes(id)}
                disabled={!catalog.notificationTargets[id]?.enabled}
                aria-label={notificationTargetLabel(id, t)}
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
            {['prompt', 'file', 'mcp', 'condition'].includes(node.kind) && (
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
}) {
  return (
    <div className="detail-stack inspector">
      <WorkflowSettings
        draft={draft}
        catalog={catalog}
        t={t}
        onUpdateDraft={onUpdateDraft}
        onToggleNotification={onToggleNotification}
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
