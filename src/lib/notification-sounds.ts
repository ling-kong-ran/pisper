// 提示音：五种音效都用 Web Audio 现场合成，不打包音频文件（没有来源与许可证问题，也不占体积）。
// 浏览器要求先有一次用户操作才能出声：第一次按键或点击时解锁，之前的提示静默跳过。

export const NOTIFICATION_SOUNDS = ['chime', 'marimba', 'drop', 'bell', 'low'] as const
export type NotificationSound = (typeof NOTIFICATION_SOUNDS)[number]

export function isNotificationSound(value: unknown): value is NotificationSound {
  return NOTIFICATION_SOUNDS.some((sound) => sound === value)
}

type Note = {
  // 相对开始的时间与时长（秒）、频率（Hz）、音色、相对音量。
  at: number
  duration: number
  frequency: number
  type?: OscillatorType
  gain?: number
  // 结束时滑到的频率（水滴那种下滑）。
  slideTo?: number
}

// 每种音效是几个短音：起音 5ms、指数衰减，音量都压在 0.35 以内，不刺耳。
const SOUND_NOTES: Record<NotificationSound, Note[]> = {
  // 叮咚：两声上行的正弦，最常见的“有事找你”。
  chime: [
    { at: 0, duration: 0.5, frequency: 987.77 },
    { at: 0.16, duration: 0.7, frequency: 1318.51 },
  ],
  // 木琴：三个上行的短三角波，轻快，适合“做完了”。
  marimba: [
    { at: 0, duration: 0.22, frequency: 783.99, type: 'triangle' },
    { at: 0.1, duration: 0.22, frequency: 987.77, type: 'triangle' },
    { at: 0.2, duration: 0.45, frequency: 1174.66, type: 'triangle' },
  ],
  // 水滴：一声快速下滑的正弦。
  drop: [{ at: 0, duration: 0.28, frequency: 1600, slideTo: 520, gain: 0.9 }],
  // 铃：基音加两个不成倍数的泛音，余音长一些。
  bell: [
    { at: 0, duration: 1.2, frequency: 880 },
    { at: 0, duration: 0.9, frequency: 880 * 2.76, gain: 0.25 },
    { at: 0, duration: 0.6, frequency: 880 * 5.4, gain: 0.12 },
  ],
  // 低鸣：两声下行的柔和方波，听着像“出了点问题”。
  low: [
    { at: 0, duration: 0.3, frequency: 392, type: 'square', gain: 0.35 },
    { at: 0.2, duration: 0.5, frequency: 293.66, type: 'square', gain: 0.35 },
  ],
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
export function playNotificationSound(sound: NotificationSound, volume = 70) {
  const ctx = audioContext()
  if (!ctx) return false
  if (ctx.state === 'suspended') void ctx.resume().catch(() => {})
  if (ctx.state !== 'running' && ctx.state !== 'suspended') return false
  const level = Math.max(0, Math.min(100, volume)) / 100
  if (!level) return false
  const start = ctx.currentTime + 0.02
  const master = ctx.createGain()
  master.gain.value = 0.35 * level
  master.connect(ctx.destination)
  let end = start
  for (const note of SOUND_NOTES[sound]) {
    const at = start + note.at
    const stop = at + note.duration
    const oscillator = ctx.createOscillator()
    const gain = ctx.createGain()
    oscillator.type = note.type || 'sine'
    oscillator.frequency.setValueAtTime(note.frequency, at)
    if (note.slideTo) oscillator.frequency.exponentialRampToValueAtTime(note.slideTo, stop)
    gain.gain.setValueAtTime(0.0001, at)
    gain.gain.exponentialRampToValueAtTime(note.gain ?? 1, at + 0.005)
    gain.gain.exponentialRampToValueAtTime(0.0001, stop)
    oscillator.connect(gain)
    gain.connect(master)
    oscillator.start(at)
    oscillator.stop(stop + 0.02)
    end = Math.max(end, stop)
  }
  // 播完断开总音量节点，不留悬挂的节点。
  window.setTimeout(() => master.disconnect(), Math.ceil((end - ctx.currentTime + 0.1) * 1000))
  return true
}
