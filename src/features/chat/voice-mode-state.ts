// 语音对话模式的状态机类型与音频工具。
// 与 VoiceInputControl（单轮语音转文字写入草稿）不同，语音对话模式是
// 全双工循环：聆听 → 转写 → 发送 → 播报 → 再次聆听。

export type VoiceModeStage =
  'idle' | 'requesting' | 'listening' | 'transcribing' | 'thinking' | 'speaking' | 'error'

// 对话浮层里展示的一轮对话（用户说 / 助手答）。
export type VoiceTurn = {
  id: string
  role: 'user' | 'agent'
  text: string
  pending?: boolean
}

// 由 PCM 采样块计算归一化电平（0~1），驱动 Orb 波形脉动。
// 用 RMS 而非峰值，说话时的起伏更平滑；再做一次开根提亮小音量。
export function pcmLevel(samples: Float32Array): number {
  if (!samples.length) return 0
  let sum = 0
  for (let index = 0; index < samples.length; index += 1) {
    sum += samples[index] * samples[index]
  }
  const rms = Math.sqrt(sum / samples.length)
  // -45dB 以下视作环境底噪直接归零，避免静音时 Orb 仍在抖动。
  if (rms < 0.0056) return 0
  return Math.min(1, Math.sqrt(rms) * 2.6)
}

// 电平滑动平均：UI 每帧消费，避免音量值跳变导致动画闪烁。
export function createLevelSmoother(alpha = 0.35) {
  let current = 0
  return {
    push(value: number) {
      current = current * (1 - alpha) + value * alpha
      return current
    },
    reset() {
      current = 0
    },
  }
}

// 截断过长文本用于字幕区展示。
export function truncateVoiceText(text: string, max = 140) {
  const trimmed = text.trim()
  return trimmed.length > max ? `${trimmed.slice(0, max - 1)}…` : trimmed
}
