// 记忆的前端数据形状（与 /api/memory* 的响应字段一致）：页面、时间线和时间演算共用。
export type MemoryType = 'concept' | 'file' | 'risk' | 'preference' | 'decision' | 'fact' | 'task'
export type MemoryStatus = 'active' | 'stale' | 'superseded' | 'archived'
export type MemoryListStatus = 'current' | 'archived' | 'history' | 'all'

export type MemoryNode = {
  id: string
  spaceId: string
  title: string
  content: string
  type: MemoryType
  sourceType: string
  sourcePath?: string
  cwd?: string
  evidence?: string
  evidenceRole?: string
  episodeId?: string
  importance?: number
  authority?: number
  status?: MemoryStatus
  topicKey?: string
  validFrom?: string
  validTo?: string
  supersededBy?: string
  supersededAt?: string
  lastConfirmedAt?: string
  confirmCount?: number
  expiresAt?: string
  pinned?: boolean
  tainted?: boolean
  archivedAt?: string
  archiveReason?: string
  createdAt?: string
  updatedAt?: string
}

export type MemorySpace = {
  id: string
  name: string
  kind: 'global' | 'project' | 'custom' | string
  rootPath?: string
  nodeCount: number
}

export type MemoryDashboard = {
  spaces: MemorySpace[]
  selectedSpaceId: string
  nodes: MemoryNode[]
  hasMore?: boolean
  counts?: { current: number; archived: number; history: number }
}

export type MemoryEvent = {
  id: string
  at: string
  spaceId: string
  actor: string
  action: string
  memoryIds: string[]
  // 还在的记忆的标题；连到旧 Runtime 时没有。
  titles?: string[]
  reason: string
  model: string
  undoneAt: string
  undoable: boolean
}

export type MemoryDetail = {
  kind: 'fact' | 'digest' | 'episode'
  item: MemoryNode
  history: MemoryNode[]
  episode?: { id: string; sessionId: string; at: string; userText: string; assistantText: string }
  // 摘要下的事实；会话摘要是这次对话记下的那些。
  children?: { id: string; title: string; validFrom: string }[]
}

// 一次对话的摘要。对话已经不在会话目录里时 sessionAvailable 为 false。
export type MemorySessionDigest = {
  id: string
  spaceId: string
  sessionId: string
  windowStart: string
  windowEnd: string
  text: string
  sessionName: string
  sessionAvailable: boolean
}

// 时间演算的一条事实：只有标题没有正文，正文在选中时按 id 取。
export type MemoryEvolutionFact = {
  id: string
  spaceId: string
  title: string
  type: MemoryType
  topicKey: string
  importance: number
  authority: number
  status: MemoryStatus
  sourceType: string
  validFrom: string
  validTo: string
  createdAt: string
  supersededBy: string
  supersededAt: string
  archivedAt: string
  archiveReason: string
  pinned: boolean
  tainted: boolean
  confirmCount: number
  lastConfirmedAt: string
  accessCount: number
}

export type MemoryEvolutionDigest = {
  id: string
  spaceId: string
  level: string
  windowStart: string
  windowEnd: string
}

export type MemoryEvolutionData = {
  spaceId: string
  now: string
  spaces: MemorySpace[]
  facts: MemoryEvolutionFact[]
  digests: MemoryEvolutionDigest[]
  truncated: boolean
}

export const MEMORY_TYPES: MemoryType[] = [
  'concept',
  'file',
  'risk',
  'preference',
  'decision',
  'fact',
  'task',
]
