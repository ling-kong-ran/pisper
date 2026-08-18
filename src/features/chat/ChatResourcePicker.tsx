// 资源选择器：@-菜单，选择技能/工作流/工具/文件等资源注入输入框。
import { useEffect, useMemo, useState } from 'react'
import { Braces, FileText, Search, Workflow, Wrench, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import type { PluginsData } from '@/features/plugins/plugin-types'
import { toolDescription, toolName } from '@/features/plugins/tool-labels'
import { apiJson } from '@/lib/api'
import type { ResourceInvocation } from '@/types/chat'
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
      apiJson<{ workflows?: WorkflowResource[] }>('/api/workflows'),
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
  }, [open, sessionId, t])

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
        className="chat-resource-dialog w-[min(820px,calc(100vw_-_32px))] h-[min(600px,calc(100dvh_-_32px))] max-w-[820px] grid-rows-[auto_minmax(0,1fr)] overflow-hidden max-[650px]:w-[calc(100vw_-_16px)] max-[650px]:h-[calc(100dvh_-_16px)] z-[220] gap-0 p-0 ring-0"
        overlayClassName="z-[220]"
        showCloseButton={false}
      >
        <div className="chat-resource-head [&_[data-slot='dialog-title']]:text-[16px] [&_[data-slot='dialog-description']]:mt-[3px] [&_[data-slot='dialog-description']]:text-[12px] max-[650px]:p-[14px] flex items-start justify-between gap-[16px] [border-bottom:1px_solid_var(--stroke-soft)] [padding:17px_18px_14px]">
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
          <div className="chat-resource-browser flex min-w-0 min-h-0 flex-col [border-right:1px_solid_var(--stroke-soft)] p-[12px] max-[650px]:min-h-0 max-[650px]:[border-right:0] max-[650px]:[border-bottom:1px_solid_var(--stroke-soft)]">
            <Tabs
              value={category}
              onValueChange={changeCategory}
              className="chat-resource-tabs [&_[data-slot='tabs-list']]:grid [&_[data-slot='tabs-list']]:w-full [&_[data-slot='tabs-list']]:h-[36px] [&_[data-slot='tabs-list']]:grid-cols-[repeat(5,minmax(0,1fr))] [&_[data-slot='tabs-list']]:[border:1px_solid_var(--stroke-soft)] [&_[data-slot='tabs-list']]:rounded-[var(--r-sm)] [&_[data-slot='tabs-list']]:bg-[var(--surface-muted)] [&_[data-slot='tabs-trigger']]:min-w-0 [&_[data-slot='tabs-trigger']]:gap-[5px] [&_[data-slot='tabs-trigger']]:rounded-[var(--r-xs)] [&_[data-slot='tabs-trigger']]:[padding-inline:6px] [&_[data-slot='tabs-trigger']]:text-[11px] [&_[data-slot='tabs-trigger'][data-state='active']]:bg-[var(--solid)] [&_[data-slot='tabs-trigger'][data-state='active']]:text-[var(--text)] [&_[data-slot='tabs-trigger']_small]:text-[var(--text-muted)] [&_[data-slot='tabs-trigger']_small]:text-[10px] [&_[data-slot='tabs-trigger']_small]:font-[500] w-full"
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
                <TabsTrigger value="workflow">
                  {t('chat:resourcePicker.workflow')}
                  <small>{categoryCounts.workflow}</small>
                </TabsTrigger>
              </TabsList>
            </Tabs>
            <label className="chat-resource-search [&:focus-within]:border-[var(--focus)] [&:focus-within]:shadow-[0_0_0_2px_var(--focus-ring)] [&_input]:w-full [&_input]:h-[36px] [&_input]:border-0 [&_input]:[outline:0] [&_input]:bg-transparent [&_input]:text-[var(--text)] [&_input]:text-[13px] flex items-center gap-[7px] [margin-top:8px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] [padding:0_10px] text-[var(--text-muted)]">
              <Search size={15} />
              <input
                autoFocus
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={t('chat:resourcePicker.search')}
              />
            </label>
            <div className="chat-resource-list [&_>_button]:grid [&_>_button]:w-full [&_>_button]:grid-cols-[30px_minmax(0,1fr)_auto] [&_>_button]:items-center [&_>_button]:gap-[8px] [&_>_button]:[border:1px_solid_transparent] [&_>_button]:rounded-[var(--r-sm)] [&_>_button]:bg-transparent [&_>_button]:p-[8px] [&_>_button]:text-[var(--text)] [&_>_button]:text-left [&_>_button]:cursor-pointer [&_>_button:hover]:bg-[var(--surface-hover)] [&_>_button.active]:border-[var(--focus)] [&_>_button.active]:bg-[var(--blue-soft)] [&_button_>_span:nth-child(2)]:flex [&_button_>_span:nth-child(2)]:min-w-0 [&_button_>_span:nth-child(2)]:flex-col [&_button_>_span:nth-child(2)]:gap-[2px] [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_em]:text-[var(--text-muted)] [&_em]:text-[10px] [&_em]:[font-style:normal] [&_>_p]:m-[auto] [&_>_p]:text-[var(--text-muted)] [&_>_p]:text-[12px] flex min-h-0 [flex:1_1_0] flex-col gap-[3px] overflow-y-auto [overscroll-behavior:contain] [scrollbar-gutter:stable] [touch-action:pan-y] [margin-top:9px]">
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
                    <span className="list-icon [.chat-resource-list_&]:grid [.chat-resource-list_&]:w-[28px] [.chat-resource-list_&]:h-[28px] [.chat-resource-list_&]:place-items-center [.chat-resource-list_&]:rounded-[var(--r-sm)] [.chat-resource-list_&]:bg-[var(--surface-subtle)] [.chat-resource-list_&]:text-[var(--star-strong)] [.session-workflow-summary_&]:grid [.session-workflow-summary_&]:w-[28px] [.session-workflow-summary_&]:h-[28px] [.session-workflow-summary_&]:place-items-center [.session-workflow-summary_&]:rounded-[var(--r-sm)] [.session-workflow-summary_&]:bg-[var(--surface-subtle)] [.session-workflow-summary_&]:text-[var(--star-strong)] grid w-[27px] h-[27px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)] [.workflow-template-gallery_&]:grid [.workflow-template-gallery_&]:w-[32px] [.workflow-template-gallery_&]:h-[32px] [.workflow-template-gallery_&]:place-items-center [.workflow-template-gallery_&]:rounded-[var(--r-sm)] [.workflow-template-gallery_&]:bg-[var(--surface-subtle)] [.workflow-template-gallery_&]:text-[var(--star-strong)]">
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
          <div className="chat-resource-config [&_>_p]:m-[auto] [&_>_p]:text-[var(--text-muted)] [&_>_p]:text-[12px] [&_>_div:first-child]:flex [&_>_div:first-child]:flex-col [&_>_div:first-child]:gap-[4px] [&_strong]:text-[14px] [&_p]:text-[var(--text-muted)] [&_p]:text-[12px] max-[650px]:min-h-0 max-[650px]:max-h-[230px] flex min-w-0 min-h-0 flex-col gap-[16px] overflow-y-auto [overscroll-behavior:contain] [padding:18px]">
            {selected ? (
              <>
                <div>
                  <strong>{selected.name}</strong>
                  <p>{selected.description || t('chat:resourcePicker.noDescription')}</p>
                </div>
                {selected.kind === 'workflow' && selected.inputs.length > 0 && (
                  <div className="chat-resource-inputs [&_label]:grid [&_label]:gap-[5px] [&_label]:text-[12px] [&_label]:font-[600] [&_input[type='checkbox']]:w-[16px] [&_input[type='checkbox']]:h-[16px] [&_input[type='checkbox']]:[accent-color:var(--blue)] flex flex-col gap-[11px]">
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
                  <code className="w-fit max-w-[100%] [overflow-wrap:anywhere] rounded-[var(--r-xs)] bg-[var(--surface-muted)] text-[var(--text-secondary)] [padding:5px_7px] text-[11px]">
                    {selected.argumentHint}
                  </code>
                )}
                <Button
                  className="chat-resource-confirm hover:bg-[var(--star-hover)] hover:text-[var(--on-accent)] min-w-[112px] min-h-[36px] self-start bg-[var(--star)] text-[var(--on-accent)] font-[650]"
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
