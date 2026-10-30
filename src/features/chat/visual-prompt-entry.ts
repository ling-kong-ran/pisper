// 输入框“+”菜单里的“生成图片”：已配置视觉模型时向输入框插入提示模板，引导用户直接描述
// 生图 / 生视频需求；未配置时提示并跳转模型设置页，形成配置闭环。
import type { Notify } from '@/app/route-context'
import { apiJson } from '@/lib/api'

export async function openVisualPromptEntry({
  templatePrompt,
  configureHint,
  notify,
  onOpenModelSettings,
  onInsertPrompt,
}: {
  templatePrompt: string
  configureHint: string
  notify?: Notify
  onOpenModelSettings?: () => void
  onInsertPrompt: (prompt: string) => void
}) {
  try {
    const status = await apiJson<{ image: unknown | null; video: unknown | null }>(
      '/api/visual/models',
    )
    if (status.image || status.video) {
      onInsertPrompt(templatePrompt)
    } else {
      notify?.(configureHint)
      onOpenModelSettings?.()
    }
  } catch {
    // 状态查询失败不打断输入：保守插入提示模板。
    onInsertPrompt(templatePrompt)
  }
}
