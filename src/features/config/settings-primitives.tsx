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
