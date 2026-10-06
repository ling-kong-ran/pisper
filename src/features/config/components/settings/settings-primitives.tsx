// 设置页共享原语：SettingsCard / Field 等分组卡片组件，
// 保证各配置分区视觉一致。
import type { ComponentProps } from 'react'
import {
  AppCard,
  AppSectionTitle,
  AppSwitch,
  StatusBadge,
  type StatusBadgeTone,
} from '@/components/ui/app-primitives'

export type SettingsBadgeTone = StatusBadgeTone

export const SettingsCard = AppCard
export const SettingsSectionTitle = AppSectionTitle
export const SettingsBadge = StatusBadge

export function SettingsSwitch(props: ComponentProps<typeof AppSwitch>) {
  return <AppSwitch density="compact" {...props} />
}
