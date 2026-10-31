// [新建 ▾]（规格 5.1、计划 §5.4–5.5）：空白组件（填名称）或从内置模板“今日用量”“便签”开始。
// 模板只生成到工作室、不放置；空白组件的提示词生成在 S7 接上（提示词卡片），这里只建目录。
import { useState } from 'react'
import { ChevronDown, Plus } from 'lucide-react'
import { COMPONENT_NAME_MAX } from './studio-model'
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'
import type { ComponentRow } from '@/features/custom-ui/custom-ui-api'
import type { StudioTemplate } from './use-studio-actions'

export function NewComponentMenu({
  onCreate,
  className,
}: {
  // 返回新建的行（失败时为 null，提示由动作层负责）。
  onCreate: (template: StudioTemplate, name?: string) => Promise<ComponentRow | null>
  className?: string
}) {
  const { t } = useI18n()
  const [naming, setNaming] = useState(false)
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const create = async (template: StudioTemplate, value?: string) => {
    setBusy(true)
    try {
      const row = await onCreate(template, value)
      if (row) {
        setNaming(false)
        setName('')
      }
    } finally {
      setBusy(false)
    }
  }
  const trimmed = name.trim()
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            size="sm"
            variant="secondary"
            disabled={busy}
            data-component-new
            className={cn(className)}
          >
            <Plus />
            {t('custom-ui:studio.new')}
            <ChevronDown className="text-content-muted" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          <DropdownMenuItem data-component-new-blank onSelect={() => setNaming(true)}>
            {t('custom-ui:studio.newBlank')}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuLabel className="text-xs font-normal text-content-muted">
            {t('custom-ui:studio.fromTemplate')}
          </DropdownMenuLabel>
          <DropdownMenuItem
            data-component-new-template="usage-today"
            onSelect={() => void create('usage-today')}
          >
            {t('custom-ui:studio.templateUsageToday')}
          </DropdownMenuItem>
          <DropdownMenuItem
            data-component-new-template="notes"
            onSelect={() => void create('notes')}
          >
            {t('custom-ui:studio.templateNotes')}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Dialog open={naming} onOpenChange={(open) => !busy && setNaming(open)}>
        <DialogContent>
          <form
            className="contents"
            onSubmit={(event) => {
              event.preventDefault()
              if (trimmed) void create('blank', trimmed)
            }}
          >
            <DialogHeader>
              <DialogTitle>{t('custom-ui:studio.newBlankTitle')}</DialogTitle>
              <DialogDescription>{t('custom-ui:studio.newBlankHint')}</DialogDescription>
            </DialogHeader>
            <Input
              autoFocus
              value={name}
              maxLength={COMPONENT_NAME_MAX}
              aria-label={t('custom-ui:studio.componentName')}
              placeholder={t('custom-ui:studio.componentNamePlaceholder')}
              data-component-new-name
              onChange={(event) => setName(event.target.value)}
            />
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={() => setNaming(false)}
              >
                {t('custom-ui:studio.cancel')}
              </Button>
              <Button type="submit" disabled={busy || !trimmed}>
                {t('custom-ui:studio.create')}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  )
}
