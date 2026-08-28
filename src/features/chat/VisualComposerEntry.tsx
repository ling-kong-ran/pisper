// 视觉生成入口按钮：已配置视觉模型时向输入框插入提示模板，引导用户
// 直接描述生图/生视频需求；未配置时通知并跳转模型设置页，形成配置闭环。
import { ImagePlus } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'

type VisualComposerEntryProps = {
  notify?: Notify
  onOpenModelSettings?: () => void
  onInsertPrompt: (prompt: string) => void
}

export function VisualComposerEntry({
  notify,
  onOpenModelSettings,
  onInsertPrompt,
}: VisualComposerEntryProps) {
  const { t } = useI18n()
  const handleClick = async () => {
    try {
      const status = await apiJson<{ image: unknown | null; video: unknown | null }>(
        '/api/visual/models',
      )
      if (status.image || status.video) {
        onInsertPrompt(t('chat:focusSession.visualPromptTemplate'))
      } else {
        notify?.(t('chat:focusSession.configureVisualModelFirst'))
        onOpenModelSettings?.()
      }
    } catch {
      // 状态查询失败不打断输入：保守插入提示模板。
      onInsertPrompt(t('chat:focusSession.visualPromptTemplate'))
    }
  }
  return (
    <button
      type="button"
      className="visual-entry-trigger [.focus-composer_&]:h-[38px] [.focus-composer_&]:border-0 [.focus-composer_&]:rounded-[var(--r-sm)] [.focus-composer_&]:bg-[var(--surface-subtle)] [.focus-composer_&]:text-[12px] [.focus-composer_&]:w-[38px] [.focus-composer_&]:min-w-[38px] [.focus-session.has-conversation_.focus-composer_&]:w-[36px] [.focus-session.has-conversation_.focus-composer_&]:min-w-[36px] [.focus-session.has-conversation_.focus-composer_&]:h-[36px] relative grid place-items-center border-0 rounded-[var(--r-xs)] bg-transparent text-[var(--text-muted)] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--star-strong)]"
      title={t('chat:focusSession.generateImage')}
      aria-label={t('chat:focusSession.generateImage')}
      onClick={() => void handleClick()}
    >
      <ImagePlus size={16} />
    </button>
  )
}
