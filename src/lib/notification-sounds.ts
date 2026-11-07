// 提示音：五种音效都用 Web Audio 现场合成（notification-sound-synth），不打包音频文件（没有来源与许可证问题，也不占体积）。
// 浏览器要求先有一次用户操作才能出声：第一次按键或点击时解锁，之前的提示静默跳过。

export const NOTIFICATION_SOUNDS = ['chime', 'marimba', 'drop', 'bell', 'low'] as const
export type NotificationSound = (typeof NOTIFICATION_SOUNDS)[number]

export function isNotificationSound(value: unknown): value is NotificationSound {
  return NOTIFICATION_SOUNDS.some((sound) => sound === value)
}

type AudioContextConstructor = new () => AudioContext

let context: AudioContext | null = null
let unlockInstalled = false

function audioContextClass(): AudioContextConstructor | null {
  if (typeof window === 'undefined') return null
  const scope = window as unknown as {
    AudioContext?: AudioContextConstructor
    webkitAudioContext?: AudioContextConstructor
  }
  return scope.AudioContext || scope.webkitAudioContext || null
}

function audioContext() {
  if (context) return context
  const Context = audioContextClass()
  if (!Context) return null
  try {
    context = new Context()
  } catch {
    return null
  }
  return context
}

// 第一次用户操作时创建并恢复音频上下文；之后不再监听。
export function installNotificationSoundUnlock() {
  if (unlockInstalled || typeof window === 'undefined') return () => {}
  unlockInstalled = true
  const unlock = () => {
    const ctx = audioContext()
    if (ctx && ctx.state === 'suspended') void ctx.resume().catch(() => {})
    window.removeEventListener('pointerdown', unlock, true)
    window.removeEventListener('keydown', unlock, true)
  }
  window.addEventListener('pointerdown', unlock, true)
  window.addEventListener('keydown', unlock, true)
  return () => {
    unlockInstalled = false
    window.removeEventListener('pointerdown', unlock, true)
    window.removeEventListener('keydown', unlock, true)
  }
}

// 播放一种音效；volume 为 0–100。上下文还没解锁或环境不支持时什么也不做。
// 音符表和合成只在响的时候才用，按需加载，不占启动体积；解锁要在用户操作里同步完成，留在这个模块。
export function playNotificationSound(sound: NotificationSound, volume = 70) {
  const ctx = audioContext()
  if (!ctx) return
  if (ctx.state === 'suspended') void ctx.resume().catch(() => {})
  void import('@/lib/notification-sound-synth')
    .then(({ synthesizeNotificationSound }) => synthesizeNotificationSound(ctx, sound, volume))
    .catch(() => {})
}
