// 视觉生成入口按钮：已配置视觉模型时向输入框插入提示模板，引导用户
// 直接描述生图/生视频需求；未配置时通知并跳转模型设置页，形成配置闭环。
import { ImagePlus } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { ComposerActionButton } from '@/features/chat/components/composer/ComposerActionButton'
import { apiJson } from '@/lib/http/api'
import type { Notify } from '@/app/routes/route-context'

type VisualComposerEntryProps = {
  menu?: boolean
  notify?: Notify
  onOpenModelSettings?: () => void
  onInsertPrompt: (prompt: string) => void
}

export function VisualComposerEntry({
  menu = false,
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
    <ComposerActionButton
      menu={menu}
      label={t('chat:focusSession.generateImage')}
      className="visual-entry-trigger"
      onClick={() => void handleClick()}
    >
      <ImagePlus size={16} />
    </ComposerActionButton>
  )
}
