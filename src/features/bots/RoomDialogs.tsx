// 群聊页的手改简报对话框：已定一行一条；待办一行一条，开头写 [x] 表示做完，写 @名字 指定负责的人。
// 群的其他设定都在右侧“群信息”面板里（RoomPanels），改了就生效。
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { FieldLabel } from '@/components/ui/field'
import { Textarea } from '@/components/ui/textarea'
import { parseTodoLine, todoLine } from './room-presentation'
import type { RoomBrief, RoomMember, RoomTodo } from './rooms-api'

export function RoomBriefDialog({
  open,
  onOpenChange,
  brief,
  members,
  onSave,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  brief: RoomBrief | null
  members: readonly RoomMember[]
  onSave: (brief: Pick<RoomBrief, 'decisions' | 'todos'>) => Promise<void>
}) {
  const { t } = useI18n()
  const [decisions, setDecisions] = useState('')
  const [todos, setTodos] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const wasOpen = useRef(false)

  // 同上：只在打开的那一下填，轮询带来的新简报不覆盖正在改的字。
  useEffect(() => {
    if (open && !wasOpen.current) {
      setDecisions((brief?.decisions || []).join('\n'))
      setTodos((brief?.todos || []).map((todo) => todoLine(todo, members)).join('\n'))
      setError('')
    }
    wasOpen.current = open
  }, [open, brief, members])

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    setSaving(true)
    setError('')
    try {
      await onSave({
        decisions: decisions
          .split('\n')
          .map((line) => line.trim())
          .filter(Boolean),
        todos: todos
          .split('\n')
          .map((line) => parseTodoLine(line, members))
          .filter((todo): todo is RoomTodo => Boolean(todo)),
      })
      onOpenChange(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <form onSubmit={submit} className="contents">
          <DialogHeader>
            <DialogTitle>{t('bots:room.brief.editTitle')}</DialogTitle>
            <DialogDescription>{t('bots:room.brief.editDescription')}</DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <FieldLabel htmlFor="room-brief-decisions">
                {t('bots:room.brief.decisionsLabel')}
              </FieldLabel>
              <Textarea
                id="room-brief-decisions"
                rows={4}
                value={decisions}
                onChange={(event) => setDecisions(event.target.value)}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <FieldLabel htmlFor="room-brief-todos">{t('bots:room.brief.todosLabel')}</FieldLabel>
              <Textarea
                id="room-brief-todos"
                rows={5}
                value={todos}
                onChange={(event) => setTodos(event.target.value)}
              />
            </div>
            {error && (
              <p role="alert" className="m-0 text-sm text-destructive">
                {error}
              </p>
            )}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              {t('common:ui.cancel')}
            </Button>
            <Button type="submit" disabled={saving}>
              {t('bots:room.brief.save')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
