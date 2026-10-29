// “看板”标签图标：三条顶端对齐、长短不一的竖条，表示并行推进的泳道。
// 用 lucide 的同一工厂生成，描边、尺寸与其他导航图标保持一致，可直接当 LucideIcon 传递。
import { createLucideIcon } from 'lucide-react'

export const LaneBarsIcon = createLucideIcon('lane-bars', [
  ['path', { d: 'M5 3v14', key: 'lane-1' }],
  ['path', { d: 'M12 3v8', key: 'lane-2' }],
  ['path', { d: 'M19 3v18', key: 'lane-3' }],
])
