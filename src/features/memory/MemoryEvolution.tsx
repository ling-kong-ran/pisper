// 记忆年轮：按时间滑动演算记忆的生长、替换与沉淀。实现见后续提交，这里先定好接口。
export type MemoryEvolutionProps = {
  // 'all' 表示全部空间。
  spaceId: string
  // 记忆变了（新增、删除、撤销）时加一，重新取数。
  refreshKey?: number
  selectedId?: string | null
  onSelect: (id: string | null) => void
}

export function MemoryEvolution(_props: MemoryEvolutionProps) {
  return null
}

export default MemoryEvolution
