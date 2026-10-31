// 星忆的前端数据形状（与 /api/memory 的响应字段一致）与星辰类型表：页面与星图共用。
export type MemoryType = 'concept' | 'file' | 'risk' | 'preference' | 'decision' | 'fact' | 'task'
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

export const MEMORY_TYPES: MemoryType[] = [
  'concept',
  'file',
  'risk',
  'preference',
  'decision',
  'fact',
  'task',
]
