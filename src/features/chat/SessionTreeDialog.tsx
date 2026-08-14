import { useEffect, useMemo, useState } from 'react'
import {
  Bot,
  Bookmark,
  FileText,
  GitBranch,
  MessageSquare,
  Settings,
  Tag,
  User,
  Wrench,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { chatErrorMessage } from './chat-errors'
import { chatApi, type SessionTreeNode, type SessionTreeResponse } from './chat-api'

type TreeView = 'conversation' | 'branches' | 'labeled' | 'all'
type FlatTreeNode = SessionTreeNode & { depth: number; branchRelated: boolean }

const conversationKinds = new Set(['user', 'assistant', 'summary', 'compaction', 'position'])

function flattenTree(nodes: SessionTreeNode[], depth = 0, parentBranched = false): FlatTreeNode[] {
  return nodes.flatMap((node) => [
    { ...node, depth, branchRelated: parentBranched || node.branchPoint },
    ...flattenTree(node.children, depth + 1, node.children.length > 1),
  ])
}

function nodeIcon(node: SessionTreeNode) {
  if (node.kind === 'user') return User
  if (node.kind === 'assistant') return Bot
  if (node.kind === 'tool') return Wrench
  if (node.kind === 'settings') return Settings
  if (node.kind === 'label') return Tag
  if (node.kind === 'summary' || node.kind === 'compaction') return FileText
  if (node.kind === 'position') return GitBranch
  if (node.label) return Bookmark
  return MessageSquare
}

export function SessionTreeDialog({
  open,
  sessionId,
  streaming,
  onClose,
  onNavigated,
}: {
  open: boolean
  sessionId: string
  streaming: boolean
  onClose: () => void
  onNavigated: (editorText: string | null) => Promise<void> | void
}) {
  const { t, language } = useI18n()
  const [data, setData] = useState<SessionTreeResponse | null>(null)
  const [view, setView] = useState<TreeView>('conversation')
  const [selectedId, setSelectedId] = useState('')
  const [label, setLabel] = useState('')
  const [summarize, setSummarize] = useState(false)
  const [loading, setLoading] = useState(false)
  const [savingLabel, setSavingLabel] = useState(false)
  const [navigating, setNavigating] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => {
    if (!open || !sessionId) return
    let cancelled = false
    setLoading(true)
    setError('')
    void chatApi
      .getSessionTree(sessionId)
      .then((next) => {
        if (cancelled) return
        setData(next)
        setSelectedId(next.leafId || next.roots[0]?.id || '')
      })
      .catch((reason) => {
        if (!cancelled) setError(chatErrorMessage(reason))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [open, sessionId])

  const nodes = useMemo(() => flattenTree(data?.roots || []), [data])
  const visibleNodes = useMemo(() => {
    if (view === 'conversation') return nodes.filter((node) => conversationKinds.has(node.kind))
    if (view === 'branches') return nodes.filter((node) => node.branchRelated)
    if (view === 'labeled') return nodes.filter((node) => Boolean(node.label))
    return nodes
  }, [nodes, view])
  const selected = nodes.find((node) => node.id === selectedId) || null

  useEffect(() => {
    setLabel(selected?.label || '')
    setSummarize(false)
  }, [selected?.id, selected?.label])

  useEffect(() => {
    if (selectedId && visibleNodes.some((node) => node.id === selectedId)) return
    const activeNode = [...visibleNodes].reverse().find((node) => node.active)
    setSelectedId(activeNode?.id || visibleNodes[0]?.id || '')
  }, [selectedId, visibleNodes])

  const typeLabel = (node: SessionTreeNode) => {
    const known: Record<string, string> = {
      user: t('chat:sessionTree.userMessage'),
      assistant: t('chat:sessionTree.agentMessage'),
      tool: t('chat:sessionTree.toolResult'),
      summary: t('chat:sessionTree.branchSummary'),
      compaction: t('chat:sessionTree.compactionSummary'),
      settings: t('chat:sessionTree.settingsChange'),
      label: t('chat:sessionTree.labelChange'),
      position: t('chat:sessionTree.branchPosition'),
      extension: t('chat:sessionTree.extensionEntry'),
      metadata: t('chat:sessionTree.metadataEntry'),
      message: t('chat:sessionTree.message'),
    }
    return known[node.kind] || node.type
  }

  const refresh = async () => {
    setLoading(true)
    setError('')
    try {
      const next = await chatApi.getSessionTree(sessionId)
      setData(next)
      setSelectedId(next.leafId || next.roots[0]?.id || '')
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setLoading(false)
    }
  }

  const saveLabel = async () => {
    if (!selected || savingLabel) return
    setSavingLabel(true)
    setError('')
    try {
      setData(await chatApi.setSessionTreeLabel(sessionId, selected.id, label))
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setSavingLabel(false)
    }
  }

  const navigate = async () => {
    if (!selected || navigating || selected.leaf || streaming || data?.streaming) return
    setNavigating(true)
    setError('')
    try {
      const next = await chatApi.navigateSessionTree(sessionId, selected.id, summarize)
      setData(next)
      if (!next.cancelled) {
        await onNavigated(next.editorText)
        onClose()
      }
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setNavigating(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <DialogContent className="chat-resource-dialog" showCloseButton>
        <div className="chat-resource-head">
          <div>
            <DialogTitle>{t('chat:sessionTree.title')}</DialogTitle>
            <DialogDescription>
              {t('chat:sessionTree.description', {
                nodes: data?.nodeCount || 0,
                branches: data?.branchCount || 0,
              })}
            </DialogDescription>
          </div>
        </div>
        <div className="chat-resource-body">
          <div className="chat-resource-browser">
            <Tabs
              className="chat-resource-tabs"
              value={view}
              onValueChange={(value) => setView(value as TreeView)}
            >
              <TabsList>
                <TabsTrigger value="conversation">{t('chat:sessionTree.conversation')}</TabsTrigger>
                <TabsTrigger value="branches">{t('chat:sessionTree.branches')}</TabsTrigger>
                <TabsTrigger value="labeled">{t('chat:sessionTree.labeled')}</TabsTrigger>
                <TabsTrigger value="all">{t('chat:sessionTree.all')}</TabsTrigger>
              </TabsList>
            </Tabs>
            <div className="chat-resource-list" data-testid="session-tree-list">
              {visibleNodes.map((node) => {
                const Icon = nodeIcon(node)
                return (
                  <button
                    key={node.id}
                    type="button"
                    className={node.id === selectedId ? 'active' : ''}
                    style={{ paddingInlineStart: 8 + Math.min(node.depth, 8) * 12 }}
                    onClick={() => setSelectedId(node.id)}
                  >
                    <span className="list-icon">
                      <Icon size={15} />
                    </span>
                    <span>
                      <strong>{node.label || node.text || typeLabel(node)}</strong>
                      <small>{node.label && node.text ? node.text : typeLabel(node)}</small>
                    </span>
                    <em>
                      {node.leaf
                        ? t('chat:sessionTree.current')
                        : node.branchPoint
                          ? t('chat:sessionTree.fork')
                          : node.active
                            ? t('chat:sessionTree.active')
                            : ''}
                    </em>
                  </button>
                )
              })}
              {!visibleNodes.length && (
                <p>
                  {loading ? t('chat:sessionTree.loading') : error || t('chat:sessionTree.empty')}
                </p>
              )}
            </div>
          </div>
          <div className="chat-resource-config">
            {selected ? (
              <>
                <div>
                  <strong>{selected.label || typeLabel(selected)}</strong>
                  <p>{selected.text || t('chat:sessionTree.noPreview')}</p>
                </div>
                <div className="chat-resource-inputs">
                  <label>
                    <span>{t('chat:sessionTree.nodeLabel')}</span>
                    <Input
                      value={label}
                      maxLength={80}
                      placeholder={t('chat:sessionTree.labelPlaceholder')}
                      onChange={(event) => setLabel(event.target.value)}
                    />
                  </label>
                  <Button
                    variant="outline"
                    disabled={savingLabel || streaming || Boolean(data?.streaming)}
                    onClick={() => void saveLabel()}
                  >
                    <Tag size={14} />
                    {savingLabel
                      ? t('chat:sessionTree.savingLabel')
                      : t('chat:sessionTree.saveLabel')}
                  </Button>
                  <label>
                    <span>{t('chat:sessionTree.abandonedBranchSummary')}</span>
                    <input
                      type="checkbox"
                      checked={summarize}
                      disabled={selected.leaf || streaming || Boolean(data?.streaming)}
                      onChange={(event) => setSummarize(event.target.checked)}
                    />
                    <small>{t('chat:sessionTree.abandonedBranchSummaryHint')}</small>
                  </label>
                </div>
                <p>{t('chat:sessionTree.sideEffectsRemain')}</p>
                {selected.timestamp && (
                  <p>
                    {new Intl.DateTimeFormat(language, {
                      dateStyle: 'medium',
                      timeStyle: 'short',
                    }).format(new Date(selected.timestamp))}
                  </p>
                )}
                {error && <p className="danger-text">{error}</p>}
                <Button
                  className="chat-resource-confirm"
                  disabled={navigating || selected.leaf || streaming || Boolean(data?.streaming)}
                  onClick={() => void navigate()}
                >
                  <GitBranch size={15} />
                  {navigating
                    ? t('chat:sessionTree.navigating')
                    : selected.leaf
                      ? t('chat:sessionTree.currentPosition')
                      : t('chat:sessionTree.continueFromNode')}
                </Button>
              </>
            ) : (
              <p>
                {loading
                  ? t('chat:sessionTree.loading')
                  : error || t('chat:sessionTree.chooseNode')}
              </p>
            )}
            {error && !selected && (
              <Button variant="outline" onClick={() => void refresh()}>
                {t('chat:sessionTree.retry')}
              </Button>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
