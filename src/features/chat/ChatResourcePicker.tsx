// 资源选择器：@-菜单，选择技能/工作流/工具/文件等资源注入输入框。
import { useEffect, useMemo, useState } from 'react'
import { Braces, FileText, Search, Workflow, Wrench, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import type { PluginsData } from '@/features/plugins/public'
import { toolDescription, toolName } from '@/features/plugins/public'
import { apiJson } from '@/lib/api'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import type { ResourceInvocation } from '@/types/chat'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { chatApi } from './chat-api'

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

type ResourceCategory = 'all' | 'prompt' | 'skill' | 'tool' | 'workflow'

type Resource = {
  kind: Exclude<ResourceCategory, 'all'>
  id: string
  name: string
  description: string
  inputs: WorkflowInput[]
  argumentHint?: string
  invocation?: string
  sourceName?: string
}

export function ChatResourcePicker({
  open,
  sessionId,
  onClose,
  onSelect,
  onCommandSelect,
}: {
  open: boolean
  sessionId: string
  onClose: () => void
  onSelect: (invocation: ResourceInvocation) => void
  onCommandSelect: (invocation: string) => void
}) {
  const { t } = useI18n()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const workflowsAvailable = runtimeFeatureAvailable(capabilities, 'workflows')
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
      chatApi.getSessionCommands(sessionId),
      workflowsAvailable
        ? apiJson<{ workflows?: WorkflowResource[] }>('/api/workflows')
        : Promise.resolve({ workflows: [] as WorkflowResource[] }),
      apiJson<PluginsData>(`/api/plugins?sessionId=${encodeURIComponent(sessionId)}`),
    ])
      .then(([commandData, workflowData, pluginData]) => {
        if (!active) return
        const callableToolNames = new Set(pluginData.callableToolNames || [])
        setResources([
          ...(commandData.commands || []).map<Resource>((command) => ({
            kind: command.source,
            id: command.name,
            name: command.name,
            description: command.description || '',
            inputs: [],
            argumentHint: command.argumentHint,
            invocation: command.invocation,
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
  }, [open, sessionId, t, workflowsAvailable])

  const categoryCounts = useMemo(
    () => ({
      all: resources.length,
      prompt: resources.filter((resource) => resource.kind === 'prompt').length,
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
    if (selected.kind === 'prompt') {
      onCommandSelect(selected.invocation || `/${selected.name}`)
      onClose()
      return
    }
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
        className="chat-resource-dialog w-[min(820px,calc(100vw_-_32px))] h-[min(600px,calc(100dvh_-_32px))] max-w-[820px] sm:max-w-[820px] grid-rows-[auto_minmax(0,1fr)] overflow-hidden max-[650px]:w-[calc(100vw_-_16px)] max-[650px]:h-[calc(100dvh_-_16px)] z-[220] gap-0 p-0 ring-0"
        overlayClassName="z-[220]"
        showCloseButton={false}
      >
        <div className="chat-resource-head flex items-start justify-between gap-4 border-b border-stroke-soft px-[18px] pt-4 pb-3.5 max-[650px]:p-3.5 [&_[data-slot='dialog-description']]:mt-0.5 [&_[data-slot='dialog-description']]:text-xs [&_[data-slot='dialog-description']]:text-content-muted [&_[data-slot='dialog-title']]:text-base [&_[data-slot='dialog-title']]:font-semibold">
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
        <div className="chat-resource-body grid min-h-0 grid-cols-[minmax(0,1.1fr)_minmax(260px,.9fr)] overflow-hidden max-[650px]:grid-cols-[1fr] max-[650px]:grid-rows-[minmax(0,1fr)_minmax(190px,auto)] max-[650px]:overflow-hidden">
          <div className="chat-resource-browser flex min-h-0 min-w-0 flex-col border-r border-stroke-soft p-3 max-[650px]:min-h-0 max-[650px]:border-r-0 max-[650px]:border-b">
            <Tabs
              value={category}
              onValueChange={changeCategory}
              className={`chat-resource-tabs w-full [&_[data-slot='tabs-list']]:grid [&_[data-slot='tabs-list']]:h-9 [&_[data-slot='tabs-list']]:w-full [&_[data-slot='tabs-list']]:rounded-sm [&_[data-slot='tabs-list']]:border [&_[data-slot='tabs-list']]:border-stroke [&_[data-slot='tabs-list']]:bg-surface-muted [&_[data-slot='tabs-list']]:p-0.5 [&_[data-slot='tabs-trigger']]:min-w-0 [&_[data-slot='tabs-trigger']]:gap-1 [&_[data-slot='tabs-trigger']]:rounded-xs [&_[data-slot='tabs-trigger']]:px-1.5 [&_[data-slot='tabs-trigger']]:text-xs [&_[data-slot='tabs-trigger']]:text-content-secondary [&_[data-slot='tabs-trigger'][data-state='active']]:bg-key [&_[data-slot='tabs-trigger'][data-state='active']]:text-content [&_[data-slot='tabs-trigger'][data-state='active']]:shadow-key [&_[data-slot='tabs-trigger']_small]:text-xs [&_[data-slot='tabs-trigger']_small]:font-normal [&_[data-slot='tabs-trigger']_small]:text-content-muted [&_[data-slot='tabs-trigger']_small]:tabular-nums ${workflowsAvailable ? "[&_[data-slot='tabs-list']]:grid-cols-[repeat(5,minmax(0,1fr))]" : "[&_[data-slot='tabs-list']]:grid-cols-[repeat(4,minmax(0,1fr))]"}`}
            >
              <TabsList aria-label={t('chat:resourcePicker.categories')}>
                <TabsTrigger value="all">
                  {t('chat:resourcePicker.all')}
                  <small>{categoryCounts.all}</small>
                </TabsTrigger>
                <TabsTrigger value="prompt">
                  {t('chat:resourcePicker.prompt')}
                  <small>{categoryCounts.prompt}</small>
                </TabsTrigger>
                <TabsTrigger value="skill">
                  {t('chat:resourcePicker.skill')}
                  <small>{categoryCounts.skill}</small>
                </TabsTrigger>
                <TabsTrigger value="tool">
                  {t('chat:resourcePicker.tool')}
                  <small>{categoryCounts.tool}</small>
                </TabsTrigger>
                {workflowsAvailable && (
                  <TabsTrigger value="workflow">
                    {t('chat:resourcePicker.workflow')}
                    <small>{categoryCounts.workflow}</small>
                  </TabsTrigger>
                )}
              </TabsList>
            </Tabs>
            <label className="chat-resource-search mt-2 flex items-center gap-2 rounded-sm border border-control bg-panel px-2.5 text-content-muted [&:focus-within]:border-focus [&:focus-within]:ring-3 [&:focus-within]:ring-focus-ring [&_input]:h-[34px] [&_input]:w-full [&_input]:border-0 [&_input]:[outline:0]! [&_input]:bg-transparent [&_input]:text-[13px] [&_input]:text-content max-[650px]:[&_input]:h-11 max-[650px]:[&_input]:text-base">
              <Search size={15} />
              <input
                autoFocus
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={t('chat:resourcePicker.search')}
              />
            </label>
            <div className="chat-resource-list mt-2 flex min-h-0 [flex:1_1_0] flex-col gap-px overflow-y-auto [overscroll-behavior:contain] [scrollbar-gutter:stable] [touch-action:pan-y] [&_>_button]:relative [&_>_button]:grid [&_>_button]:w-full [&_>_button]:cursor-pointer [&_>_button]:grid-cols-[28px_minmax(0,1fr)_auto] [&_>_button]:items-center [&_>_button]:gap-2 [&_>_button]:rounded-sm [&_>_button]:px-2 [&_>_button]:py-1.5 [&_>_button]:text-left [&_>_button]:text-content [&_>_button:hover]:bg-surface-hover [&_>_button.active]:bg-surface-hover [&_>_button.active]:before:absolute [&_>_button.active]:before:inset-y-1.5 [&_>_button.active]:before:left-0 [&_>_button.active]:before:w-[3px] [&_>_button.active]:before:rounded-full [&_>_button.active]:before:bg-star max-[650px]:[&_>_button]:min-h-11 [&_button_>_span:nth-child(2)]:flex [&_button_>_span:nth-child(2)]:min-w-0 [&_button_>_span:nth-child(2)]:flex-col [&_strong]:truncate [&_strong]:text-[13px] [&_strong]:font-medium [&_small]:truncate [&_small]:text-xs [&_small]:text-content-muted [&_em]:text-xs [&_em]:whitespace-nowrap [&_em]:text-content-muted [&_em]:not-italic [&_>_p]:m-auto [&_>_p]:text-xs [&_>_p]:text-content-muted">
              {visible.map((resource) => {
                const Icon =
                  resource.kind === 'prompt'
                    ? FileText
                    : resource.kind === 'skill'
                      ? Braces
                      : resource.kind === 'tool'
                        ? Wrench
                        : Workflow
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
                    <span className="list-icon grid size-7 place-items-center rounded-sm bg-surface-muted text-content-secondary">
                      <Icon size={15} />
                    </span>
                    <span>
                      <strong>{resource.name}</strong>
                      <small>{resource.description}</small>
                    </span>
                    <em>
                      {resource.kind === 'prompt'
                        ? t('chat:resourcePicker.prompt')
                        : resource.kind === 'skill'
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
          <div className="chat-resource-config flex min-h-0 min-w-0 flex-col gap-4 overflow-y-auto overscroll-contain p-[18px] max-[650px]:max-h-[230px] max-[650px]:min-h-0 [&_>_div:first-child]:flex [&_>_div:first-child]:flex-col [&_>_div:first-child]:gap-1 [&_>_p]:m-auto [&_>_p]:text-xs [&_>_p]:text-content-muted [&_p]:text-xs [&_p]:text-content-muted [&_strong]:text-[15px] [&_strong]:font-medium">
            {selected ? (
              <>
                <div>
                  <strong>{selected.name}</strong>
                  <p>{selected.description || t('chat:resourcePicker.noDescription')}</p>
                </div>
                {selected.kind === 'workflow' && selected.inputs.length > 0 && (
                  <div className="chat-resource-inputs flex flex-col gap-3 [&_input[type='checkbox']]:size-4 [&_input[type='checkbox']]:[accent-color:var(--star)] [&_label]:grid [&_label]:gap-1.5 [&_label]:text-xs [&_label]:font-medium [&_label]:text-content-secondary">
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
                {selected.kind === 'prompt' && selected.argumentHint && (
                  <code className="w-fit max-w-full rounded-xs bg-inline-code px-1.5 py-1 font-mono text-xs [overflow-wrap:anywhere] text-content-secondary">
                    {selected.argumentHint}
                  </code>
                )}
                <Button
                  className="chat-resource-confirm min-h-8 min-w-[112px] self-start bg-star text-[var(--on-accent)] hover:bg-star-hover max-[650px]:min-h-11"
                  onClick={confirm}
                >
                  {selected.kind === 'prompt'
                    ? t('chat:resourcePicker.insertPrompt')
                    : t('chat:resourcePicker.useResource')}
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
