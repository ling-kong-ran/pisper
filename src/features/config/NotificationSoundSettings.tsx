// 设置 › 通知 › 提示音：需要你、跑完、失败各用哪种音效（或不响），音量，Pisper 在前台时响不响。
// 偏好按设备记（notification-sound-store），不随系统通知开关变化：系统通知关着也可以只听声音。
import { useState } from 'react'
import { Play } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Translate } from '@/app/navigation'
import { AppSelect } from '@/components/AppSelect'
import { SettingsRow, SettingsSection } from '@/components/app/SettingsRow'
import { Button } from '@/components/ui/button'
import { Slider } from '@/components/ui/slider'
import { Switch } from '@/components/ui/switch'
import {
  NOTIFICATION_SOUNDS,
  playNotificationSound,
  type NotificationSound,
} from '@/lib/notification-sounds'
import {
  NOTIFICATION_SOUND_EVENTS,
  useNotificationSoundStore,
  type NotificationSoundChoice,
  type NotificationSoundEvent,
} from '@/stores/notification-sound-store'

function soundLabel(t: Translate, sound: NotificationSound) {
  switch (sound) {
    case 'chime':
      return t('config:notificationSounds.soundChime')
    case 'marimba':
      return t('config:notificationSounds.soundMarimba')
    case 'drop':
      return t('config:notificationSounds.soundDrop')
    case 'bell':
      return t('config:notificationSounds.soundBell')
    default:
      return t('config:notificationSounds.soundLow')
  }
}

function eventLabel(t: Translate, event: NotificationSoundEvent) {
  if (event === 'waiting') return t('config:notificationSounds.eventWaiting')
  if (event === 'finished') return t('config:notificationSounds.eventFinished')
  return t('config:notificationSounds.eventFailed')
}

function eventHint(t: Translate, event: NotificationSoundEvent) {
  if (event === 'waiting') return t('config:notificationSounds.eventWaitingHint')
  if (event === 'finished') return t('config:notificationSounds.eventFinishedHint')
  return t('config:notificationSounds.eventFailedHint')
}

export function NotificationSoundSettings() {
  const { t } = useI18n()
  const preference = useNotificationSoundStore()
  // 拖动时先显示新音量，松手再保存并试听一声。
  const [volumeDraft, setVolumeDraft] = useState<number | null>(null)
  const volume = volumeDraft ?? preference.volume
  const preview = (sound: NotificationSoundChoice) => {
    if (sound !== 'none') playNotificationSound(sound, preference.volume)
  }
  return (
    <SettingsSection
      data-config-card="notifications-sounds"
      title={t('config:notificationSounds.title')}
      description={t('config:notificationSounds.description')}
    >
      <SettingsRow
        title={t('config:notificationSounds.enabled')}
        control={
          <Switch
            checked={preference.enabled}
            aria-label={t('config:notificationSounds.enabled')}
            onCheckedChange={(enabled) => preference.update({ enabled })}
          />
        }
      />
      {NOTIFICATION_SOUND_EVENTS.map((event) => {
        const sound = preference.sounds[event]
        return (
          <SettingsRow
            key={event}
            data-sound-event={event}
            title={eventLabel(t, event)}
            description={eventHint(t, event)}
            control={
              <div className="flex items-center gap-2">
                <AppSelect
                  value={sound}
                  className="w-32"
                  aria-label={eventLabel(t, event)}
                  disabled={!preference.enabled}
                  onChange={(change) => {
                    const next = change.target.value as NotificationSoundChoice
                    preference.setSound(event, next)
                    preview(next)
                  }}
                >
                  <option value="none">{t('config:notificationSounds.soundNone')}</option>
                  {NOTIFICATION_SOUNDS.map((item) => (
                    <option key={item} value={item}>
                      {soundLabel(t, item)}
                    </option>
                  ))}
                </AppSelect>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="pointer-coarse:size-11"
                  disabled={!preference.enabled || sound === 'none'}
                  aria-label={t('config:notificationSounds.preview', {
                    event: eventLabel(t, event),
                  })}
                  title={t('config:notificationSounds.preview', { event: eventLabel(t, event) })}
                  onClick={() => preview(sound)}
                >
                  <Play aria-hidden="true" />
                </Button>
              </div>
            }
          />
        )
      })}
      <SettingsRow
        title={t('config:notificationSounds.volume')}
        control={
          <div className="flex w-48 items-center gap-3">
            <Slider
              min={0}
              max={100}
              step={5}
              value={[volume]}
              disabled={!preference.enabled}
              aria-label={t('config:notificationSounds.volume')}
              valueText={`${volume}%`}
              onValueChange={([next]) => setVolumeDraft(next)}
              onValueCommit={([next]) => {
                setVolumeDraft(null)
                preference.update({ volume: next })
                const sample = preference.sounds.waiting
                if (sample !== 'none') playNotificationSound(sample, next)
              }}
            />
            <span className="w-9 text-right text-xs text-content-muted tabular-nums">
              {volume}%
            </span>
          </div>
        }
      />
      <SettingsRow
        title={t('config:notificationSounds.whileFocused')}
        description={t('config:notificationSounds.whileFocusedHint')}
        control={
          <Switch
            checked={preference.whileFocused}
            disabled={!preference.enabled}
            aria-label={t('config:notificationSounds.whileFocused')}
            onCheckedChange={(whileFocused) => preference.update({ whileFocused })}
          />
        }
      />
    </SettingsSection>
  )
}
