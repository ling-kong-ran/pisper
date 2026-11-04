// 提示音偏好：哪些时刻响、各用哪种音效、音量、Pisper 在前台时响不响。按设备记（每台电脑、手机的
// 声音环境不同），存在 'pisper-notification-sounds'，经 page-state-storage 落盘并同步到 Runtime 镜像。
import { create } from 'zustand'
import { STORAGE_KEYS } from '@/app/storage'
import { isNotificationSound, type NotificationSound } from '@/lib/notification-sounds'
import { pageStateStorage } from '@/lib/page-state-storage'

export const NOTIFICATION_SOUND_EVENTS = ['waiting', 'finished', 'failed'] as const
export type NotificationSoundEvent = (typeof NOTIFICATION_SOUND_EVENTS)[number]
// 'none' 表示这个时刻不响。
export type NotificationSoundChoice = NotificationSound | 'none'

export type NotificationSoundPreference = {
  enabled: boolean
  volume: number
  // Pisper 在前台（窗口可见且有焦点）时也响；关掉后只在你不看着时响。
  whileFocused: boolean
  sounds: Record<NotificationSoundEvent, NotificationSoundChoice>
}

export const DEFAULT_NOTIFICATION_SOUND_PREFERENCE: Readonly<NotificationSoundPreference> =
  Object.freeze({
    enabled: true,
    volume: 70,
    whileFocused: true,
    sounds: Object.freeze({ waiting: 'chime', finished: 'marimba', failed: 'low' }),
  })

function choice(value: unknown, fallback: NotificationSoundChoice): NotificationSoundChoice {
  return value === 'none' || isNotificationSound(value) ? value : fallback
}

// 存储内容按不可信输入解析：坏 JSON、错类型、未知字段一律回到默认值，永不抛错。
export function parseNotificationSoundPreference(raw: unknown): NotificationSoundPreference {
  const defaults = DEFAULT_NOTIFICATION_SOUND_PREFERENCE
  let value: unknown = raw
  if (typeof raw === 'string') {
    try {
      value = JSON.parse(raw)
    } catch {
      value = null
    }
  }
  const source =
    value && typeof value === 'object' && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : {}
  const sounds =
    source.sounds && typeof source.sounds === 'object' && !Array.isArray(source.sounds)
      ? (source.sounds as Record<string, unknown>)
      : {}
  const volume = Number(source.volume)
  return {
    enabled: typeof source.enabled === 'boolean' ? source.enabled : defaults.enabled,
    volume: Number.isFinite(volume)
      ? Math.round(Math.max(0, Math.min(100, volume)))
      : defaults.volume,
    whileFocused:
      typeof source.whileFocused === 'boolean' ? source.whileFocused : defaults.whileFocused,
    sounds: {
      waiting: choice(sounds.waiting, defaults.sounds.waiting),
      finished: choice(sounds.finished, defaults.sounds.finished),
      failed: choice(sounds.failed, defaults.sounds.failed),
    },
  }
}

function readPreference(): NotificationSoundPreference {
  try {
    return parseNotificationSoundPreference(
      pageStateStorage.getItem(STORAGE_KEYS.notificationSounds),
    )
  } catch {
    return parseNotificationSoundPreference(null)
  }
}

type NotificationSoundState = NotificationSoundPreference & {
  update: (patch: Partial<Omit<NotificationSoundPreference, 'sounds'>>) => void
  setSound: (event: NotificationSoundEvent, sound: NotificationSoundChoice) => void
}

export const useNotificationSoundStore = create<NotificationSoundState>((set, get) => {
  const save = (next: NotificationSoundPreference) => {
    set(next)
    // 存储不可用（隐私模式、配额）时照常生效，只是下次启动不保留。
    try {
      pageStateStorage.setItem(STORAGE_KEYS.notificationSounds, JSON.stringify(next))
    } catch {
      // 忽略：提示音偏好不是关键数据。
    }
  }
  const current = (): NotificationSoundPreference => {
    const { enabled, volume, whileFocused, sounds } = get()
    return { enabled, volume, whileFocused, sounds }
  }
  return {
    ...readPreference(),
    update: (patch) => save(parseNotificationSoundPreference({ ...current(), ...patch })),
    setSound: (event, sound) =>
      save(
        parseNotificationSoundPreference({
          ...current(),
          sounds: { ...current().sounds, [event]: sound },
        }),
      ),
  }
})
