import { useEffect, useMemo, useState } from 'react'
import { Braces, Search, Workflow, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
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

type Resource =
  | { kind: 'skill'; id: string; name: string; description: string; inputs: WorkflowInput[] }
  | { kind: 'workflow'; id: string; name: string; description: string; inputs: WorkflowInput[] }

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
  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState<Resource | null>(null)
  const [argumentsValue, setArgumentsValue] = useState<Record<string, unknown>>({})
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (!open) return
    let active = true
    setLoading(true)
    Promise.all([
      apiJson<{ skills?: SkillResource[] }>(
        `/api/skills?sessionId=${encodeURIComponent(sessionId)}`,
      ),
      apiJson<{ workflows?: WorkflowResource[] }>('/api/workflows'),
    ])
      .then(([skillData, workflowData]) => {
        if (!active) return
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
        ])
      })
      .finally(() => active && setLoading(false))
    return () => {
      active = false
    }
  }, [open, sessionId])

  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase()
    return resources.filter(
      (resource) =>
        !needle || `${resource.name} ${resource.description}`.toLowerCase().includes(needle),
    )
  }, [query, resources])

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
      <DialogContent className="chat-resource-dialog gap-0 p-0 ring-0" showCloseButton={false}>
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
                const Icon = resource.kind === 'skill' ? Braces : Workflow
                return (
                  <button
                    type="button"
                    className={selected?.id === resource.id ? 'active' : ''}
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
                      {resource.kind === 'skill' ? 'Skill' : t('chat:resourcePicker.workflow')}
                    </em>
                  </button>
                )
              })}
              {!visible.length && (
                <p>{loading ? t('chat:resourcePicker.loading') : t('chat:resourcePicker.empty')}</p>
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
                <Button onClick={confirm}>{t('chat:resourcePicker.useResource')}</Button>
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
