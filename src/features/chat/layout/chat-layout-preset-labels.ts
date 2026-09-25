import type { ChatLayoutPreset } from './chat-layout'

type PresetLabel = { title: string; description: string }

// 设置编辑器与会话选择器共用展示文案，保持此入口轻量且每个翻译键都可静态检查。
export function chatLayoutPresetLabels(
  t: (key: string) => string,
): Record<ChatLayoutPreset['id'], PresetLabel> {
  return {
    default: {
      title: t('chat-layout:layout.presetDefault'),
      description: t('chat-layout:layout.presetDefaultHint'),
    },
    focus: {
      title: t('chat-layout:layout.presetFocus'),
      description: t('chat-layout:layout.presetFocusHint'),
    },
    workbench: {
      title: t('chat-layout:layout.presetWorkbench'),
      description: t('chat-layout:layout.presetWorkbenchHint'),
    },
    studio: {
      title: t('chat-layout:layout.presetStudio'),
      description: t('chat-layout:layout.presetStudioHint'),
    },
  }
}
