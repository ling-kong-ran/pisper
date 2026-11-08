// 删除 Bot：设置页最下面的危险操作，确认后删掉 Bot 本身、它的定时任务和记忆；
// 它的会话留下来变成普通会话（之后可以像别的会话一样归档）。会话页头菜单的“删除 Bot”跳到这里打开确认。
import { useState } from 'react'
import { Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import { deleteBot, type Bot } from './bots-api'
import { useBotsStore } from './bots-store'

export function BotDeleteSection({
  bot,
  initiallyOpen = false,
  onDeleted,
  onError,
}: {
  bot: Bot
  initiallyOpen?: boolean
  onDeleted: () => void
  onError: (message: string) => void
}) {
  const { t } = useI18n()
  const remove = useBotsStore((state) => state.remove)
  const [open, setOpen] = useState(initiallyOpen)
  const [deleting, setDeleting] = useState(false)

  const confirm = async () => {
    setDeleting(true)
    try {
      await deleteBot(bot.id)
      remove(bot.id)
      setOpen(false)
      onDeleted()
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setDeleting(false)
    }
  }

  return (
    <section className="flex items-center justify-between gap-3 border-t border-stroke pt-4">
      <div className="min-w-0">
        <div className="text-sm text-content">{t('bots:delete.title')}</div>
        <p className="text-xs text-content-muted">{t('bots:delete.hint')}</p>
      </div>
      <Button
        type="button"
        variant="destructive"
        size="sm"
        data-bot-delete={bot.id}
        onClick={() => setOpen(true)}
      >
        <Trash2 aria-hidden="true" />
        {t('bots:delete.action')}
      </Button>
      <AlertDialog open={open} onOpenChange={(next) => !deleting && setOpen(next)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t('bots:delete.confirmTitle', { name: bot.name })}</AlertDialogTitle>
            <AlertDialogDescription>{t('bots:delete.confirmDescription')}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleting}>{t('common:ui.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={deleting}
              onClick={(event) => {
                // 删除完成前不关对话框：失败时留在原处给出错误。
                event.preventDefault()
                void confirm()
              }}
            >
              {deleting && <OrbitSpinner className="text-current" />}
              {t('bots:delete.action')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  )
}
