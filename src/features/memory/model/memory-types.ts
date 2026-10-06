// 记忆领域类型：空间、节点、链接和候选条目。
export type MemoryType = 'concept' | 'file' | 'risk' | 'preference' | 'decision' | 'fact' | 'task'

export type MemorySpace = {
  id: string
  name: string
  kind: 'global' | 'custom' | string
  nodeCount: number
}

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
  importance?: number
  authority?: number
  createdAt?: string
}

export type MemoryLink = { id: string; sourceId: string; targetId: string }

export type MemoryCandidate = {
  id: string
  spaceId: string
  title: string
  content: string
  evidence?: string
  sourceType?: string
  confidence?: number
  topicKey?: string
  createdAt?: string
  expiresAt?: string
}

export type MemoryData = {
  spaces: MemorySpace[]
  nodes: MemoryNode[]
  links: MemoryLink[]
  candidates: MemoryCandidate[]
  selectedSpaceId: string
}

export type GalaxyStar = {
  node: MemoryNode
  x: number
  y: number
  twinkle: number
}

export type GalaxyPoint = Pick<GalaxyStar, 'x' | 'y'>
