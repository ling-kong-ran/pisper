// “看板”标签图标：三条顶端对齐、长短不一的竖条，表示并行推进的泳道。
// 用 lucide 的同一工厂生成，描边、尺寸与其他导航图标保持一致，可直接当 LucideIcon 传递。
import { createLucideIcon } from 'lucide-react'

export const LaneBarsIcon = createLucideIcon('lane-bars', [
  ['path', { d: 'M5 3v14', key: 'lane-1' }],
  ['path', { d: 'M12 3v8', key: 'lane-2' }],
  ['path', { d: 'M19 3v18', key: 'lane-3' }],
])

// “会话”标签图标（主页改为会话后的第一个一级标签）：一个对话气泡。和看板图标放在同一个模块，
// 标题栏常驻时不必为一枚小图标多拆一个入口分块（入口体积预算很紧）。
export const SessionBubbleIcon = createLucideIcon('session-bubble', [
  ['path', { d: 'M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z', key: 'bubble' }],
])
