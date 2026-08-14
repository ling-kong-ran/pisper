import { useEffect, useMemo, useState } from 'react'
import { Braces, Search, Workflow, Wrench, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import type { PluginsData } from '@/features/plugins/plugin-types'
import { toolDescription, toolName } from '@/features/plugins/tool-labels'
import { apiJson } from '@/lib/api'
import type { ResourceInvocation } from '@/types/chat'

type SkillResource = {
  id: string
  name: string
  description?: string
  enabled?: boolean
  command?: string
}
type WorkflowInput = {
  id: string
  name: string
  label: string
  type: string
  required: boolean
  defaultValue: unknown
}
type WorkflowResource = {
  id: string
  name: string
  description?: string
  status: string
  revision?: number
  inputs?: WorkflowInput[]
}

type ResourceCategory = 'all' | 'skill' | 'tool' | 'workflow'

type Resource =
  | { kind: 'skill'; id: string; name: string; description: string; inputs: WorkflowInput[] }
  | { kind: 'workflow'; id: string; name: string; description: string; inputs: WorkflowInput[] }
  | {
      kind: 'tool'
      id: string
      name: string
      description: string
      inputs: WorkflowInput[]
      sourceName: string
    }

export function ChatResourcePicker({
  open,
  sessionId,
  onClose,
  onSelect,
}: {
  open: boolean
  sessionId: string
  onClose: () => void
  onSelect: (invocation: ResourceInvocation) => void
}) {
  const { t } = useI18n()
  const [resources, setResources] = useState<Resource[]>([])
  const [category, setCategory] = useState<ResourceCategory>('all')
  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState<Resource | null>(null)
  const [argumentsValue, setArgumentsValue] = useState<Record<string, unknown>>({})
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => {
    if (!open) return
    let active = true
    setCategory('all')
    setQuery('')
    setSelected(null)
    setArgumentsValue({})
    setLoading(true)
    setError('')
    Promise.all([
      apiJson<{ skills?: SkillResource[] }>(
        `/api/skills?sessionId=${encodeURIComponent(sessionId)}`,
      ),
      apiJson<{ workflows?: WorkflowResource[] }>('/api/workflows'),
      apiJson<PluginsData>(`/api/plugins?sessionId=${encodeURIComponent(sessionId)}`),
    ])
      .then(([skillData, workflowData, pluginData]) => {
        if (!active) return
        const callableToolNames = new Set(pluginData.callableToolNames || [])
        setResources([
          ...(skillData.skills || [])
            .filter((skill) => skill.enabled && skill.command)
            .map<Resource>((skill) => ({
              kind: 'skill',
              id: skill.id,
              name: skill.name,
              description: skill.description || '',
              inputs: [],
            })),
          ...(workflowData.workflows || [])
            .filter((workflow) => workflow.status === 'published')
            .map<Resource>((workflow) => ({
              kind: 'workflow',
              id: workflow.id,
              name: workflow.name,
              description: workflow.description || '',
              inputs: workflow.inputs || [],
            })),
          ...(pluginData.plugins || []).flatMap((plugin) =>
            (plugin.capabilities || [])
              .filter((capability) => callableToolNames.has(capability.name))
              .map<Resource>((capability) => ({
                kind: 'tool',
                id: capability.name,
                name: capability.id
                  ? toolName(capability, t)
                  : String(capability.label || capability.name),
                description: capability.id
                  ? toolDescription(capability, t)
                  : capability.description,
                inputs: [],
                sourceName: plugin.name,
              })),
          ),
        ])
      })
      .catch((caught) => {
        if (!active) return
        setResources([])
        setError(caught instanceof Error ? caught.message : String(caught))
      })
      .finally(() => active && setLoading(false))
    return () => {
      active = false
    }
  }, [open, sessionId, t])

  const categoryCounts = useMemo(
    () => ({
      all: resources.length,
      skill: resources.filter((resource) => resource.kind === 'skill').length,
      tool: resources.filter((resource) => resource.kind === 'tool').length,
      workflow: resources.filter((resource) => resource.kind === 'workflow').length,
    }),
    [resources],
  )

  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase()
    return resources.filter(
      (resource) =>
        (category === 'all' || resource.kind === category) &&
        (!needle ||
          `${resource.name} ${resource.id} ${resource.description} ${resource.kind === 'tool' ? resource.sourceName : ''}`
            .toLowerCase()
            .includes(needle)),
    )
  }, [category, query, resources])

  const changeCategory = (value: string) => {
    const next = value as ResourceCategory
    setCategory(next)
    if (selected && next !== 'all' && selected.kind !== next) setSelected(null)
  }

  const choose = (resource: Resource) => {
    setSelected(resource)
    setArgumentsValue(
      Object.fromEntries(resource.inputs.map((input) => [input.name, input.defaultValue ?? ''])),
    )
  }

  const confirm = () => {
    if (!selected) return
    onSelect({
      kind: selected.kind,
      resourceId: selected.id,
      resourceName: selected.name,
      arguments: selected.kind === 'workflow' ? argumentsValue : undefined,
      behavior: selected.kind === 'workflow' ? 'background' : 'foreground',
    })
    onClose()
  }

  return (
    <Dialog open={open} onOpenChange={(value) => !value && onClose()}>
      <DialogContent
        className="chat-resource-dialog z-[220] gap-0 p-0 ring-0"
        overlayClassName="z-[220]"
        showCloseButton={false}
      >
        <div className="chat-resource-head">
          <div>
            <DialogTitle>{t('chat:resourcePicker.title')}</DialogTitle>
            <DialogDescription>{t('chat:resourcePicker.description')}</DialogDescription>
          </div>
          <Button
            size="icon-sm"
            variant="ghost"
            aria-label={t('chat:resourcePicker.close')}
            onClick={onClose}
          >
            <X />
          </Button>
        </div>
        <div className="chat-resource-body">
          <div className="chat-resource-browser">
            <Tabs value={category} onValueChange={changeCategory} className="chat-resource-tabs">
              <TabsList aria-label={t('chat:resourcePicker.categories')}>
                <TabsTrigger value="all">
                  {t('chat:resourcePicker.all')}
                  <small>{categoryCounts.all}</small>
                </TabsTrigger>
                <TabsTrigger value="skill">
                  {t('chat:resourcePicker.skill')}
                  <small>{categoryCounts.skill}</small>
                </TabsTrigger>
                <TabsTrigger value="tool">
                  {t('chat:resourcePicker.tool')}
                  <small>{categoryCounts.tool}</small>
                </TabsTrigger>
                <TabsTrigger value="workflow">
                  {t('chat:resourcePicker.workflow')}
                  <small>{categoryCounts.workflow}</small>
                </TabsTrigger>
              </TabsList>
            </Tabs>
            <label className="chat-resource-search">
              <Search size={15} />
              <input
                autoFocus
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={t('chat:resourcePicker.search')}
              />
            </label>
            <div className="chat-resource-list">
              {visible.map((resource) => {
                const Icon =
                  resource.kind === 'skill' ? Braces : resource.kind === 'tool' ? Wrench : Workflow
                return (
                  <button
                    type="button"
                    className={
                      selected?.kind === resource.kind && selected.id === resource.id
                        ? 'active'
                        : ''
                    }
                    key={`${resource.kind}:${resource.id}`}
                    onClick={() => choose(resource)}
                  >
                    <span className="list-icon">
                      <Icon size={15} />
                    </span>
                    <span>
                      <strong>{resource.name}</strong>
                      <small>{resource.description}</small>
                    </span>
                    <em>
                      {resource.kind === 'skill'
                        ? 'Skill'
                        : resource.kind === 'tool'
                          ? t('chat:resourcePicker.tool')
                          : t('chat:resourcePicker.workflow')}
                    </em>
                  </button>
                )
              })}
              {!visible.length && (
                <p>
                  {loading
                    ? t('chat:resourcePicker.loading')
                    : error || t('chat:resourcePicker.empty')}
                </p>
              )}
            </div>
          </div>
          <div className="chat-resource-config">
            {selected ? (
              <>
                <div>
                  <strong>{selected.name}</strong>
                  <p>{selected.description || t('chat:resourcePicker.noDescription')}</p>
                </div>
                {selected.kind === 'workflow' && selected.inputs.length > 0 && (
                  <div className="chat-resource-inputs">
                    {selected.inputs.map((input) => (
                      <label key={input.id}>
                        <span>
                          {input.label}
                          {input.required ? ' *' : ''}
                        </span>
                        {input.type === 'boolean' ? (
                          <input
                            type="checkbox"
                            checked={Boolean(argumentsValue[input.name])}
                            onChange={(event) =>
                              setArgumentsValue((current) => ({
                                ...current,
                                [input.name]: event.target.checked,
                              }))
                            }
                          />
                        ) : (
                          <Input
                            type={input.type === 'number' ? 'number' : 'text'}
                            required={input.required}
                            value={String(argumentsValue[input.name] ?? '')}
                            onChange={(event) =>
                              setArgumentsValue((current) => ({
                                ...current,
                                [input.name]: event.target.value,
                              }))
                            }
                          />
                        )}
                      </label>
                    ))}
                  </div>
                )}
                <Button className="chat-resource-confirm" onClick={confirm}>
                  {t('chat:resourcePicker.useResource')}
                </Button>
              </>
            ) : (
              <p>{t('chat:resourcePicker.chooseResource')}</p>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
